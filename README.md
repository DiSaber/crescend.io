# CS 4398 Project

## Backend

### Prerequisites:

- Rust: [https://rust-lang.org/tools/install](https://rust-lang.org/tools/install)
- (Windows) C++ Build Tools: The Rust installer should automatically prompt to install. Otherwise follow: [https://rust-lang.github.io/rustup/installation/windows-msvc.html](https://rust-lang.github.io/rustup/installation/windows-msvc.html)

### Running

First cd to the backend directory

```bash
cd backend
```

Set the environment variables below, then run with

```bash
cargo run
```

or for release builds (optimization)

```bash
cargo run --release
```

### API documentation

With the backend running, open [Scalar](http://localhost:3000/scalar) to explore and try the existing endpoints.

### Environment variables

For development, put these required variables in `backend/.env`:

| Variable | Value |
| --- | --- |
| `GOOGLE_CLIENT_ID` | Google OAuth client ID |
| `GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `GOOGLE_REDIRECT_URI` | Registered callback URL (local: `http://localhost:3000/api/auth/google/callback`) |
| `JWT_SIGNING_SECRET` | Random signing secret, at least 32 bytes |
