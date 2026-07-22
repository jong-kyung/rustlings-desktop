# Rustlings Desktop

An **unofficial** desktop GUI for [rustlings](https://github.com/rust-lang/rustlings), the official Rust exercises. Browse the exercise curriculum, edit and run exercises, and track your progress — all in a native macOS app.

> This project is not affiliated with the Rust project or the rustlings maintainers.

## Install

**Homebrew (recommended):**

```sh
brew install jong-kyung/tap/rustlings-desktop
```

**Direct download:** grab the latest `.dmg` from the [Releases page](https://github.com/jong-kyung/rustlings-desktop/releases) and drag the app to Applications.

You'll need the Rust toolchain (`rustup` / `cargo`) installed to run exercises — the app checks for it on startup and guides you through setup if it's missing.

## First launch

The app is not signed with an Apple Developer ID, so macOS Gatekeeper will block it the first time:

1. Double-click **Rustlings Desktop** — macOS shows a warning and refuses to open it. Close the dialog.
2. Open **System Settings → Privacy & Security**.
3. Scroll down to the message about "Rustlings Desktop" and click **Open Anyway**.
4. Confirm in the dialog that follows. This is only needed once.

<details>
<summary>Advanced alternatives</summary>

Skip quarantine at install time with Homebrew:

```sh
brew install --cask --no-quarantine jong-kyung/tap/rustlings-desktop
```

If macOS reports the app is **"damaged and can't be opened"** (common with unsigned dmg installs), remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine "/Applications/Rustlings Desktop.app"
```

</details>
