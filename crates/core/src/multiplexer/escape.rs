//! Shutdown of a session host that speaks another protocol [`VERSION`].
//!
//! After an upgrade `endpoint.json` can still name the old host, which cannot
//! decode the current enums. These frames are encoded by hand so that they
//! never change with them. Hosts without `mutual_auth` (VERSION 1, and
//! VERSION 2 before 1.0.0-rc.5) accept only `Hello` and `Shutdown`; every
//! later host accepts `Authenticate`, `Proof` and `Shutdown`, and every future
//! host must keep accepting them (see the frozen variants documented on
//! `protocol::Request`). A frame is a 4-byte little-endian body length, then a
//! fixint bincode body that starts with the variant index as a `u32`.
//!
//! [`VERSION`]: super::protocol::VERSION
use super::protocol::{client_proof, constant_time_eq, host_proof};
use anyhow::{bail, ensure};
use rand_core::{OsRng, RngCore};
use std::io::{Read, Write};

const REQUEST_HELLO: u32 = 0;
const REQUEST_SHUTDOWN: u32 = 10;
const REQUEST_AUTHENTICATE: u32 = 14;
const REQUEST_PROOF: u32 = 15;
const RESPONSE_OK: u32 = 0;
const RESPONSE_ERROR: u32 = 5;
const RESPONSE_CHALLENGE: u32 = 8;
/// Hosts read replies before authentication with this limit, and so do we.
const MAX_REPLY_BYTES: usize = 4096;

fn frame(variant: u32, fields: &[&[u8]]) -> Vec<u8> {
    let length = 4 + fields.iter().map(|field| field.len()).sum::<usize>();
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(&(length as u32).to_le_bytes());
    frame.extend_from_slice(&variant.to_le_bytes());
    for field in fields {
        frame.extend_from_slice(field);
    }
    frame
}

/// `Request::Authenticate { version, client_nonce }`.
pub(crate) fn authenticate(version: u32, client_nonce: &[u8; 32]) -> Vec<u8> {
    frame(
        REQUEST_AUTHENTICATE,
        &[&version.to_le_bytes(), client_nonce],
    )
}

/// `Request::Proof(proof)`.
pub(crate) fn proof(proof: &[u8; 32]) -> Vec<u8> {
    frame(REQUEST_PROOF, &[proof])
}

/// `Request::Shutdown`.
pub(crate) fn shutdown() -> Vec<u8> {
    frame(REQUEST_SHUTDOWN, &[])
}

/// The retired `Request::Hello { version, token }`, which sends the token
/// itself: the only greeting of hosts without `mutual_auth` (VERSION 1, and
/// VERSION 2 before 1.0.0-rc.5).
pub(crate) fn hello(version: u32, token: &str) -> Vec<u8> {
    frame(
        REQUEST_HELLO,
        &[
            &version.to_le_bytes(),
            &(token.len() as u64).to_le_bytes(),
            token.as_bytes(),
        ],
    )
}

/// `Response::Ok`.
#[cfg(test)]
pub(crate) fn ok() -> Vec<u8> {
    frame(RESPONSE_OK, &[])
}

/// `Response::Error(message)`.
#[cfg(test)]
pub(crate) fn error(message: &str) -> Vec<u8> {
    frame(
        RESPONSE_ERROR,
        &[&(message.len() as u64).to_le_bytes(), message.as_bytes()],
    )
}

/// `Response::Challenge { server_nonce, server_proof }`.
#[cfg(test)]
pub(crate) fn challenge(server_nonce: &[u8; 32], server_proof: &[u8; 32]) -> Vec<u8> {
    frame(RESPONSE_CHALLENGE, &[server_nonce, server_proof])
}

pub(crate) enum Reply {
    Ok,
    Error(String),
    Challenge {
        server_nonce: [u8; 32],
        server_proof: [u8; 32],
    },
}

pub(crate) fn read_reply(reader: &mut impl Read) -> anyhow::Result<Reply> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(
        (4..=MAX_REPLY_BYTES).contains(&length),
        "invalid session host reply"
    );
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let (variant, fields) = body.split_at(4);
    Ok(match u32::from_le_bytes(variant.try_into()?) {
        RESPONSE_OK if fields.is_empty() => Reply::Ok,
        RESPONSE_ERROR if fields.len() >= 8 => {
            let (length, message) = fields.split_at(8);
            ensure!(
                u64::from_le_bytes(length.try_into()?) == message.len() as u64,
                "invalid session host reply"
            );
            Reply::Error(String::from_utf8_lossy(message).into_owned())
        }
        RESPONSE_CHALLENGE if fields.len() == 64 => Reply::Challenge {
            server_nonce: fields[..32].try_into()?,
            server_proof: fields[32..].try_into()?,
        },
        _ => bail!("unexpected session host reply"),
    })
}

fn expect_ok(stream: &mut impl Read) -> anyhow::Result<()> {
    match read_reply(stream)? {
        Reply::Ok => Ok(()),
        Reply::Error(error) => bail!(error),
        Reply::Challenge { .. } => bail!("unexpected session host reply"),
    }
}

/// Authenticates to a host of protocol `version` and asks it to shut down,
/// which ends all of its terminals. With `mutual_auth` the host proves it
/// knows `token` before we prove it in turn. Without it, the host only
/// accepts the token itself: the caller must have checked that the host still
/// holds `host.lock`, so the port cannot belong to another process.
pub(crate) fn shutdown_host(
    stream: &mut (impl Read + Write),
    version: u32,
    token: &str,
    mutual_auth: bool,
) -> anyhow::Result<()> {
    if mutual_auth {
        let mut client_nonce = [0; 32];
        OsRng.fill_bytes(&mut client_nonce);
        stream.write_all(&authenticate(version, &client_nonce))?;
        let (server_nonce, server_proof) = match read_reply(stream)? {
            Reply::Challenge {
                server_nonce,
                server_proof,
            } => (server_nonce, server_proof),
            Reply::Error(error) => bail!(error),
            Reply::Ok => bail!("unexpected session host reply"),
        };
        ensure!(
            constant_time_eq(
                &server_proof,
                &host_proof(token, &client_nonce, &server_nonce)
            ),
            "the multiplexer endpoint failed authentication; it is not this user's session host"
        );
        stream.write_all(&proof(&client_proof(token, &client_nonce, &server_nonce)))?;
    } else {
        stream.write_all(&hello(version, token))?;
    }
    expect_ok(stream)?;
    stream.write_all(&shutdown())?;
    stream.flush()?;
    expect_ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::protocol::{Request, Response, read_message, write_message};

    fn encode(message: &impl serde::Serialize) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_message(&mut bytes, message).unwrap();
        bytes
    }

    /// Later protocol versions may change anything except these frames;
    /// otherwise a newer client can no longer stop this host (audit B1).
    #[test]
    fn frozen_frames_match_the_wire_enums() {
        let nonce = [3; 32];
        let other = [9; 32];
        assert_eq!(
            encode(&Request::Authenticate {
                version: 7,
                client_nonce: nonce
            }),
            authenticate(7, &nonce)
        );
        assert_eq!(encode(&Request::Proof(nonce)), proof(&nonce));
        assert_eq!(encode(&Request::Shutdown), shutdown());
        assert_eq!(encode(&Response::Ok), ok());
        assert_eq!(encode(&Response::Error("no".into())), error("no"));
        assert_eq!(
            encode(&Response::Challenge {
                server_nonce: nonce,
                server_proof: other,
            }),
            challenge(&nonce, &other)
        );
        // The reply decoder reads what the current host writes.
        assert!(matches!(
            read_reply(&mut encode(&Response::Ok).as_slice()).unwrap(),
            Reply::Ok
        ));
        assert!(matches!(
            read_reply(&mut encode(&Response::Error("no".into())).as_slice()).unwrap(),
            Reply::Error(message) if message == "no"
        ));
        assert!(matches!(
            read_reply(&mut challenge(&nonce, &other).as_slice()).unwrap(),
            Reply::Challenge { server_nonce, server_proof }
                if server_nonce == nonce && server_proof == other
        ));
        // The current host still decodes the frames it must accept forever.
        assert!(matches!(
            read_message(&mut authenticate(7, &nonce).as_slice()).unwrap(),
            Request::Authenticate { version: 7, client_nonce } if client_nonce == nonce
        ));
        assert!(matches!(
            read_message(&mut shutdown().as_slice()).unwrap(),
            Request::Shutdown
        ));
    }

    /// The retired greeting, as VERSION 1 and early VERSION 2 hosts decode it.
    #[test]
    fn hello_frame_matches_the_retired_wire_shape() {
        #[derive(serde::Serialize)]
        enum Retired {
            Hello { version: u32, token: String },
        }
        assert_eq!(
            encode(&Retired::Hello {
                version: 2,
                token: "secret".into()
            }),
            hello(2, "secret")
        );
    }

    #[test]
    fn malformed_replies_are_rejected() {
        for bytes in [
            vec![0, 0, 0, 0],
            5000u32.to_le_bytes().to_vec(),
            frame(RESPONSE_OK, &[&[1]]),
            frame(RESPONSE_CHALLENGE, &[&[0; 63]]),
            frame(RESPONSE_ERROR, &[&9u64.to_le_bytes(), b"short"]),
            frame(2, &[]),
        ] {
            assert!(read_reply(&mut bytes.as_slice()).is_err());
        }
    }
}
