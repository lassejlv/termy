---
title: Mobile access
description: Pair the Termy iPhone app to browse, view and control this Mac's sessions, end-to-end encrypted.
---

## Pair an iPhone

1. Open **Settings → Mobile**, enable mobile access, and choose **Pair iPhone…**.
2. Scan the QR code in the Termy iPhone app, and confirm the computer and relay there.
3. Approve the phone on your Mac.

No account or terminal command is needed. Paired phones can browse, view, create, control and end this Mac's sessions. Remove a phone's access in the same settings pane.

## What must be running

Termy Next must be running, with its session host enabled, and your Mac must be awake and reachable. Closing all windows keeps mobile access available. Quitting, or turning mobile access off, stops remote access without ending your programs.

## Your own relay

The default relay is `https://relay.termy.sh`. To use your own server:

1. Deploy the [relay stack](https://github.com/lassejlv/termy/blob/main/crates/relay/README.md#self-host-on-a-vps).
2. Enter its HTTPS URL in **Mobile → Relay Server** and click **Apply**.
3. Pair again. The QR code sets the phone's relay for you.

> Sessions stay end-to-end encrypted with either relay.
