# Phone view

A web page of your features, for a phone. It shows:

- every open feature, those waiting for you first
- each feature's stage, its stages, what it waits for, its history and its stage files (brief, plan, …)
- each agent's state (working, waiting for you, idle) and the last lines of its pane, refreshed every 2 s

It reads the feature files on disk and asks herdr.

## What it can do

- **Stage page buttons.** A feature's page has the same buttons as the desktop stage page, like Continue, Plan fixes, Run checks and Restart. Buttons that only show or open something on the desktop are left out, as is Ship. A brief or plan waiting for review also gets Approve.
- **Send back.** A brief or plan waiting for review can go back to its agent with your comment.
- **Findings.** AI review findings are listed with Fix, Dismiss (with a reason) and Reopen.
- **Agents.** An agent's page has a reply box (one line, then Enter) and keys for its menus: Esc, 1, 2, 3, ↑, ↓ and Enter.

The phone asks before every stage button. gerdr does the press as if you clicked on the desktop and its messages show on the phone. If gerdr asks for a confirmation, the phone asks you instead. The desktop dialog closes again, so nothing waits on an empty desk.

Presses need a running gerdr. The phone view finds it through a socket in `$XDG_RUNTIME_DIR/gerdr/`. Without gerdr the page still shows everything, with no buttons. Typing to an agent goes straight to herdr.

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
