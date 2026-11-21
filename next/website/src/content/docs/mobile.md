---
title: Mobile access
description: Pair the Termy iPhone app to browse, view and control this Mac's sessions, end-to-end encrypted.
---

## Pair an iPhone

1. Open **Settings → Mobile**, enable mobile access, and choose **Pair iPhone…**.
2. Scan the QR code in the Termy iPhone app.
3. Approve the phone on your Mac.

No account, server or terminal command is needed. Paired phones can browse, view, create, control and end this Mac's sessions. Remove a phone's access in the same settings pane.

## How the iPhone connects

The iPhone connects to this Mac directly: on the same Wi-Fi or Ethernet network, or over [Tailscale](https://tailscale.com) when both devices are on your tailnet, which works from anywhere. Nothing sits in between, and sessions are end-to-end encrypted. **Settings → Mobile → Connection** shows the addresses your iPhone can use.

Notifications about finished commands arrive while Termy is open on the iPhone.

## What must be running

Termy Next must be running, with its session host enabled, and your Mac must be awake and reachable. Closing all windows keeps mobile access available. Quitting, or turning mobile access off, stops remote access without ending your programs.

## iPhones paired through the relay

Earlier versions connected through a relay server, which is gone. iPhones paired that way are listed under **Paired Through the Relay**: pair them again, and remove the old entry.
