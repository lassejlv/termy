---
title: Agents and activity
description: See which agent needs you. Programs report their state, and tabs and the Activity popup show it.
---

## Program status

Activity comes only from [OSC 7501](/docs/osc-7501), the Program Status Protocol. Claude Code and other agents report through it. Nothing is guessed from the screen or the process tree.

- **Working:** the tab ends in a three-dot loader and shows the program's reported title. Hover the tab to swap the dots for its close button.
- **Blocked:** a blue dot on the tab icon. The program needs you, for example to grant a permission.
- **Done:** a green dot, which clears once you've looked at the pane.
- **Error:** a red dot.

## The Activity popup

Press **⌘.** or click the eye button. It lists every reporting program across all sessions, with what it needs ("Needs permission") and its message. Click a row to jump to that pane.

## Notifications

Termy Next can notify you when a program needs you. You're only notified about panes you aren't looking at, and clicking a notification takes you to the pane. Turn notifications on in Settings or in the [welcome tour](/docs/welcome-tour).

> Programs that don't report status show no state at all. A plain build or test run still appears by name in its tab.
