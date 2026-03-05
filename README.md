# MyLittleBotty

## Install Latest Release Binary

Run:

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/refs/heads/main/startup/install.sh | bash
```

Notes:
- Currently supports macOS only.
- Installs binary to `~/.mylittlebotty/bin` and appends PATH in your shell profile.
- After install, restart shell (or `source ~/.zshrc`) and run: `mylittlebotty`

## Uninstall

Run:

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/refs/heads/main/startup/uninstall.sh | bash
```

Notes:
- Removes `~/.mylittlebotty` (including binary).
- Removes PATH lines added by installer from `~/.zshrc`, `~/.bash_profile`, `~/.bashrc`.
