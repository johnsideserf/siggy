# FAQ

## Does siggy replace the Signal phone app?

No. siggy runs as a **linked device**, just like Signal Desktop. Your phone
remains the primary device and must stay registered. siggy connects through
signal-cli, which registers as a secondary device on your account.

## Can I use siggy without a phone?

No. Signal requires a phone number for registration and a primary device. siggy
links to your existing account as a secondary device.

## Is my data encrypted?

Messages are end-to-end encrypted in transit by the Signal protocol (handled by
signal-cli). Locally, messages are stored in an unencrypted SQLite database --
the same approach used by Signal Desktop. If you want zero local persistence,
use `--incognito` mode. See the [Security](security.md) page for full details
and recommendations.

## Can I send files and images?

Yes. Use `/attach` to open a file browser and select a file to send. Received
images are rendered inline, and other files are saved to your download directory.

## Does it work on Windows?

Yes. Pre-built Windows binaries are provided in each release. Use a modern
terminal like Windows Terminal for the best experience (clickable links, proper
Unicode, truecolor support).

## Does it work over SSH?

Yes. siggy is a terminal application and works perfectly over SSH sessions.
Make sure signal-cli and Java are available on the remote machine.

## Can I use multiple Signal accounts?

Yes. Use the `-a` flag or config file to specify which account to use:

```sh
siggy -a +15551234567
siggy -a +15559876543
```

Each account needs its own device linking via signal-cli.

## I locked the session and forgot my passphrase. Now what?

Quit siggy (or kill the process if the lock screen is in the way) and run:

```sh
siggy --reset-lock
```

The flag deletes the stored passphrase hash file and prints the path it
removed. The next time you `/lock`, you will be prompted to set a fresh
passphrase. By design there is no in-app recovery -- the lock is a
casual-snooping deterrent, not protection against file-system access.

## Images aren't rendering as native pixels inside tmux

tmux strips the Kitty / iTerm2 graphics escapes unless you opt into
passthrough, and `TERM_PROGRAM` becomes `tmux` so siggy cannot auto-detect
the outer terminal. Two steps:

1. In `~/.tmux.conf`: `set -g allow-passthrough on` (requires tmux 3.3+).
2. Launch siggy with the outer terminal's protocol named:
   `SIGGY_IMAGE_PROTOCOL=kitty siggy` (or `iterm2`, `sixel`, `halfblock`).

See the Troubleshooting page for details.

## How do I update siggy?

Re-run the install script, or download the latest binary from the
[Releases page](https://github.com/johnsideserf/siggy/releases).

If you installed from source:

```sh
cargo install --git https://github.com/johnsideserf/siggy.git --force
```

## What license is siggy under?

[GPL-3.0](https://github.com/johnsideserf/siggy/blob/master/LICENSE).
This is a copyleft license -- forks must remain open source under the same terms.
