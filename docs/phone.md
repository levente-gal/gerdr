# Phone view

A read-only web page of your features, for a phone. It shows:

- every open feature, those waiting for you first
- each feature's stage, its stages, what it waits for, its history and its stage files (brief, plan, …)
- each agent's state (working, waiting for you, idle) and the last lines of its pane, refreshed every 2 s

It changes nothing. It reads the feature files on disk and asks herdr.

## How it is reached

`gerdr --phone [port]` serves the page on `127.0.0.1:8390` (or the given port). It runs on its own and opens no window. Nothing outside the machine can reach that address. `tailscale serve` puts it on your tailnet over HTTPS, so only your own devices reach it.

The server answers only requests for `localhost`, `127.0.0.1` or a `*.ts.net` name. A web page that points a name of its own at your machine gets `421`.

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

- Only this machine's features and agents. Agents running on another machine aren't listed.
- A feature at Manual test doesn't show what it waits for.
- Pane colors come from a script on cdn.jsdelivr.net. Without it, the text shows plain.
