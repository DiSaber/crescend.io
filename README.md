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

Then you can run with

```bash
cargo run
```

or for release builds (optimization)

```bash
cargo run --release
```

### YouTube integration

The backend exposes `POST /api/youtube/metadata` for guest add-song flows.
Send a JSON body containing a YouTube watch, short, embed, or `youtu.be` URL:

```json
{ "url": "https://youtu.be/dQw4w9WgXcQ" }
```

The response normalizes the link and returns the video ID, oEmbed title/author
metadata, thumbnail, and an embeddable player URL. The client can use
`video_id` as the queue's stable identity, render `embed_url` in the player,
and request the next queue item when the YouTube player reports `ENDED`.
Playlist URLs and non-YouTube URLs are rejected with `422`.
