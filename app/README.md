# VPN on Demand – app source

Tauri 2 app. Logic is in Rust (`src-tauri/src/`), the UI (`src/`, `index.html`) only displays state.

```bash
npm install
npm run tauri dev                      # run on this OS
(cd src-tauri && cargo test)           # unit tests
```

See the repository's top-level README for what the app does and how to set it up.
