# humchord

Record a hum on Linux (PipeWire / Steinberg CI1), then:

- spectrogram of the take with pitch contour and identified notes
- clip in/out handles
- all chord voicings (inversions, open, drop 2/3, shells) — click to hear
- **Progressions** tab: I–IV–V–I, pop axis, ii–V–I, blues, minor cadences, …

```
humchord           # GUI
humchord --cli     # terminal
humchord take.wav  # file analysis
```

Requires PipeWire (`pw-record` / `pw-cat`). Build with Rust:

```
cargo install --path .
```
