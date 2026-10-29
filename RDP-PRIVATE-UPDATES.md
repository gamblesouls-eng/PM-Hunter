# RDP private test auto-updates

This branch builds a Windows test installer from the `rdp-private-test-update` branch. It checks only unpublished draft releases named `RDP private test build N` with tags `rdp-private-test-N`; it never falls back to the public Poopihunt update channel.

## One-time setup on each RDP

1. While logged into your GitHub account, open Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token.
2. Set Resource owner to `gamblesouls-eng`, repository access to **Only select repositories → PM-Hunter**, and **Contents: Read-only**. No write permissions are needed. Give it an expiration that suits you.
3. Copy the token into a plain-text file named `update-token.txt` in `%LOCALAPPDATA%\Hunt\` (typically `C:\Users\<you>\AppData\Local\Hunt\update-token.txt`). The file must contain only the token, with no quotes or other text. Never commit it to GitHub or send it to anyone.
4. Restart Poopihunt. This test build checks for a newer draft release when it starts and when the hunt stops; when found, it verifies the SHA-256 and runs the installer automatically.

The token stays on that RDP in the local Hunt data folder. The installer leaves this folder intact across updates. Since the test release remains a draft, unauthenticated users cannot list or download it. Revoke the token in GitHub settings if it is ever exposed.

## Publishing a test update

Push code to this branch. The Windows workflow builds it with the run number as its version, calculates the installer hash, and creates an unpublished draft release. Do not publish that release; the updater intentionally consumes draft releases only.
