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
signal-cli). Locally, messages are stored in a plain SQLite database. If you want
zero local persistence, use `--incognito` mode.

## Can I send files and images?

Currently, siggy can **receive** attachments (images are rendered inline,
other files are saved to disk). **Sending** attachments is on the
[roadmap](../dev-guide/roadmap.md).

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
