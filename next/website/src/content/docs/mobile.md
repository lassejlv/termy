---
title: Mobile access
description: Pair the Termy iPhone app to browse, view and control this Mac's sessions, end-to-end encrypted.
---

## Pair an iPhone

1. Start [Tailscale](https://tailscale.com) on this Mac and on your iPhone, signed in to the same tailnet.
2. Open **Settings → Mobile**, enable mobile access, and choose **Pair iPhone…**.
3. Scan the QR code in the Termy iPhone app.
4. Approve the phone on your Mac.

No account, server or terminal command is needed. Paired phones can browse, view, create, control and end this Mac's sessions. Remove a phone's access in the same settings pane.

## How the iPhone connects

The iPhone connects to this Mac directly over Tailscale, from anywhere, as long as Tailscale runs on both devices on the same tailnet. Nothing sits in between, and sessions are end-to-end encrypted. Being on the same Wi-Fi isn't enough: without Tailscale, Termy Next says to start it. **Settings → Mobile → Connection** shows the Tailscale address your iPhone uses.

Notifications about finished commands arrive while Termy is open on the iPhone.

## What must be running

Termy Next must be running, with its session host enabled, and your Mac must be awake and reachable. Closing all windows keeps mobile access available. Quitting, or turning mobile access off, stops remote access without ending your programs.

## iPhones paired through the relay

Earlier versions connected through a relay server, which is gone. iPhones paired that way are listed under **Paired Through the Relay**: pair them again, and remove the old entry.
