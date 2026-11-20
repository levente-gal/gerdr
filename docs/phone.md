# Phone view

A web page of your features, for a phone. It shows:

- every open feature, those waiting for you first, as its sidebar card shows it: stage dots, the stage's line, and a coloured edge for working, waiting or failed
- each feature's rail as its feature tab draws it (each stage's mark, short line and agent, and the line back to Build), and what it waits for
- the page of the stage it is at, as the desktop's stage page says it in words: its agent, the stage's file to read (the brief, the plan, the final plan), Build's progress (tap it for the milestones, ticked as they are made), fix rounds (folded: tap one for what it fixes), Verify's checks, AI review's findings, the Ship check. Tap a stage on the rail for its page. A stage's other files (decisions, research, the build log) are links
- its history, folded: the last event shows, tap for all
- each agent's state (working, waiting for you, idle) and the last lines of its pane, refreshed every 2 s

What runs blinks: a working agent's dot, and the stage dot or rail node at work. It stays still when the phone asks for reduced motion.

It reads the feature files on disk and asks herdr. The cards, the rail and the stage pages come from the running gerdr; without one, a stage's page shows its file alone.

## What it can do

- **Stage page buttons.** A feature's page has the same buttons as the desktop stage page, like Continue, Plan fixes, Run checks and Restart. Buttons that only show or open something on the desktop are left out, as is Ship. A brief or plan waiting for review also gets Approve.
- **Send back.** A brief or plan waiting for review can go back to its agent with your comment.
- **Findings.** AI review findings are listed with Fix, Dismiss (with a reason) and Reopen.
- **Agents.** An agent's page has a reply box (one line, then Enter) and keys for its menus: Esc, 1, 2, 3, ↑, ↓ and Enter.

The phone asks before every stage button. gerdr does the press as if you clicked on the desktop and its messages show on the phone. If gerdr asks for a confirmation, the phone asks you instead. The desktop dialog closes again, so nothing waits on an empty desk.

Presses need a running gerdr. The phone view finds it through a socket in `$XDG_RUNTIME_DIR/gerdr/`. Without gerdr the page still shows everything, with no buttons. Typing to an agent goes straight to herdr.

## Notifications

The phone can get a notification when, while you're away from the desk:

- an agent needs you, or finishes;
- a brief or plan is ready for review;
- a pull request's checks finish.

Tap one to open its feature or agent; a CI one opens the pull request. Turn them on at the bottom of the feature list, with Turn on; Send a test checks they arrive. Each phone turns them on for itself. On an iPhone, open gerdr from the home screen first: Safari gives a page notifications only there (iOS 16.4 or later).

Away means no key or pointer on the desktop for 2 minutes, as niri (or another host with ext-idle-notify) tells gerdr; a host that can't tell counts input to gerdr only. While you're away, an agent on screen counts as unseen, so the desktop notifies of it too. An agent that still needs you is reminded of on the phone at most every 15 minutes.

gerdr sends the notifications itself, through `curl` to the push service of the phone's browser (Apple's or Google's). Each is encrypted for that phone; the push service sees only where it goes. A phone's address and gerdr's key are in `~/.local/share/gerdr/phone/`. A notification that couldn't go shows on the "!" badge.

## Phone connections in gerdr

While the phone view runs, gerdr's footer shows a phone chip. The chip says "Phone view" with a grey dot when no phone is connected, and the phone's name with a green dot when one is ("Phone: pixel-8"). A phone counts as connected for 15 s after its last request. The page asks every 2 to 5 s while it shows, so a phone with gerdr open counts as connected, and one in a pocket doesn't.

Click the chip, or pick Phone connections in the launcher, to open the Phone connections dialog:

- **Phone view**: the server, its port and how long it has been up. Stop, Restart, or Start when it's stopped. Start and Restart go through the `gerdr-phone` user service. A server started by hand can only be stopped.
- **One row per device** that has opened the page, the last to ask first. The row shows the device's tailnet name (`tailscale whois`), or what its browser runs on, its tailnet address, its Tailscale login, and when it was last seen.
  - **Block** answers that device with 403 until you unblock it, and turns its notifications off.
  - **Notifications off** stops sending it notifications. Its page shows Turn on again.
  - **Forget** takes the device off the list and turns its notifications off. It comes back if it opens the page again.

A device is known by the address `tailscale serve` passes on. Requests made on this machine without Tailscale aren't listed. The list is kept in `~/.local/share/gerdr/phone/devices.json`. If gerdr can't read it, the dialog says so, and the phone view refuses every phone until the file is fixed or deleted, so that a blocked phone isn't let in.

## How it is reached

`gerdr --phone [port]` serves the page on `127.0.0.1:8390` (or the given port). It runs on its own and opens no window. Nothing outside the machine can reach that address. `tailscale serve` puts it on your tailnet over HTTPS, so only your own devices reach it.

The server answers only requests for `localhost`, `127.0.0.1` or a `*.ts.net` name. A web page that points a name of its own at your machine gets `421`. A press must carry the `X-Gerdr: phone` header, which only the phone view's own page sends. A form on another site can't, so it gets `403`.

Anyone on your tailnet can press buttons and type to your agents, and an agent can run commands on your machine. Keep the tailnet to your own devices.

## Set up the desktop (once)

1. Install Tailscale and sign in:

   ```sh
   sudo dnf install tailscale
   sudo systemctl enable --now tailscaled
   sudo tailscale up
   sudo tailscale set --operator=$USER
   ```

2. In the Tailscale admin console, under DNS, turn on MagicDNS and HTTPS certificates.

3. Run the phone view as a user service. Save this as `~/.config/systemd/user/gerdr-phone.service`:

   ```ini
   [Unit]
   Description=gerdr phone view

   [Service]
   ExecStart=%h/.local/bin/gerdr --phone
   Restart=on-failure

   [Install]
   WantedBy=default.target
   ```

   Then start it:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now gerdr-phone
   ```

   Its log: `journalctl --user -u gerdr-phone`. After `install.sh`, run `systemctl --user restart gerdr-phone` to pick up the new binary.

4. Put it on the tailnet:

   ```sh
   tailscale serve --bg 8390
   ```

   `tailscale serve status` prints the address, like `https://desk.tail1234.ts.net`.

## Set up a phone (once)

1. Install the Tailscale app and sign in with the same account.
2. Open the address from `tailscale serve status`.
3. Add it to the home screen. It then opens full screen, like an app.
   - iPhone: Safari, Share, Add to Home Screen.
   - Android: Chrome, the ⋮ menu, Add to Home screen (or Install app).

## Limits

- Only this machine's features and agents. Agents running on another machine aren't listed. A feature another machine writes can't be acted on from the phone.
- A reply to an agent is one line. Line breaks become spaces.
- A feature at Manual test doesn't show what it waits for.
- Pane colors come from a script on cdn.jsdelivr.net. Without it, the text shows plain.
- Notifications need a running gerdr, and only come while you're away. An agent that finishes in the 2 minutes after you left isn't notified on the phone.
