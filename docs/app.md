# app

The egui interface, the custom dark theme, and the async to UI bridge. This is
where the typed pieces become something a person uses.

Source: [`src/app.rs`](../src/app.rs)

## The shape of an egui app

egui is an immediate mode UI library: there is no retained widget tree. Every
frame, eframe calls one method and you describe the entire interface from
scratch based on your current state. Our type plugs into that loop with a trait
impl:

```rust
impl eframe::App for CompressorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) { ... }
}
```

`update` runs many times per second. Because the whole UI is redrawn from state
each frame, the cleanest way to model the screen is an explicit state machine,
which is exactly what we do.

## The state machine

```rust
enum UiState {
    Idle,
    Loading,
    Result(Compression),
    Error(String),
}
```

An enum sum type makes the four possible screens mutually exclusive. You cannot
be loading and showing a result at the same time, because the value is one
variant at a time. The `output` method matches on `&self.state` and the compiler
forces all four arms to be handled.

The app struct owns everything the UI needs:

```rust
pub struct CompressorApp {
    runtime: tokio::runtime::Runtime,
    product_text: String,
    audience_text: String,
    state: UiState,
    rx: Option<Receiver<Result<Compression, EngineError>>>,
}
```

`product_text` and `audience_text` are the buffers bound to the two text fields.
`rx` is `Some` only while a request is in flight.

## The async to UI bridge

This is the core of keeping the interface responsive. The network call is
asynchronous and can take seconds; the UI thread must never wait on it. The
bridge has three parts.

**1. A tokio runtime owned by the app.** Built once in `new`:

```rust
let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(2)
    .enable_all()
    .build()
    .expect("failed to start the tokio runtime");
```

`enable_all` turns on the IO and timer drivers reqwest needs.

**2. An `mpsc` channel.** When the user submits, we create a
`std::sync::mpsc` channel, keep the receiver, and move the sender into a spawned
task:

```rust
let (tx, rx) = std::sync::mpsc::channel();
self.rx = Some(rx);
self.state = UiState::Loading;

let ctx = ctx.clone();
self.runtime.spawn(async move {
    let result = engine::compress(product, audience).await;
    let _ = tx.send(result);
    ctx.request_repaint();
});
```

The channel decouples the worker thread from the UI thread. The task runs
`engine::compress`, sends the result back, and then calls `request_repaint` on a
**clone** of the egui context. `egui::Context` is `Clone` and cheap to clone (it
is a handle); cloning it lets the task wake the UI exactly when the result is
ready, instead of the UI busy polling.

**3. A non blocking poll once per frame.** At the top of every `update`:

```rust
fn poll_result(&mut self) {
    let Some(rx) = &self.rx else { return; };
    match rx.try_recv() {
        Ok(result) => { /* transition to Result or Error */ }
        Err(TryRecvError::Empty) => {}          // still working
        Err(TryRecvError::Disconnected) => { /* worker died */ }
    }
}
```

`try_recv` returns immediately whether or not a value is ready. If the channel
is empty, we do nothing and the next frame checks again. If a value arrived, we
move into `Result` or `Error` and drop the receiver.

The flow end to end: `submit` validates the text into `domain` newtypes (a bad
input goes straight to `Error` without touching the network), spawns the task,
and sets `Loading`. Later frames poll, and when the result lands the state
machine advances and the result renders.

## The custom theme

The look is defined once, in `install_theme`, called at startup. It is built from
named design tokens so the styling lives in one place.

**Color tokens.** A deep, slightly blue charcoal base, layered surfaces, hairline
borders, two text weights, and a single confident accent (a warm coral) used
sparingly:

```rust
const BG: Color32          = Color32::from_rgb(0x0F, 0x11, 0x16);
const SURFACE: Color32     = Color32::from_rgb(0x16, 0x19, 0x20);
const SURFACE_ALT: Color32 = Color32::from_rgb(0x1E, 0x22, 0x2B);
const BORDER: Color32      = Color32::from_rgb(0x2A, 0x2F, 0x3A);
const TEXT: Color32        = Color32::from_rgb(0xE6, 0xE8, 0xEC);
const MUTED: Color32       = Color32::from_rgb(0x97, 0x9F, 0xAD);
const ACCENT: Color32      = Color32::from_rgb(0xFF, 0x6B, 0x3D);
```

The accent appears in exactly three places: the short rule under the title, the
primary Compress button, and small markers (the promise eyebrow and the headline
numbers). Restraint is the point. One accent, used where it carries meaning.

**Spacing scale.** A single rhythm for all gaps:

```rust
const SP_XS: f32 = 4.0;  const SP_SM: f32 = 8.0;  const SP_MD: f32 = 16.0;
const SP_LG: f32 = 24.0; const SP_XL: f32 = 32.0;
```

**Type scale.** Concrete sizes mapped onto egui's semantic text styles
(`Heading`, `Body`, `Small`, `Monospace`) plus explicit sizes for the display
title and the uppercase eyebrow labels.

**Surface treatment.** Every widget state gets rounded corners, the surface fill,
and the hairline border, so nothing looks like a default egui control. The text
selection color is a translucent accent.

These are applied by cloning the current `Style`, mutating its `text_styles`,
`visuals`, and `spacing`, and calling `ctx.set_style(style)`.

## Layout and composition

`update` draws a single `CentralPanel` with the background fill and generous
margins, wrapped in a vertical `ScrollArea` so the result scrolls when it is
tall. The content is three sections drawn in order: `header`, `inputs`, and
`output`.

The recurring surface is the `card` helper, which takes a closure for its body:

```rust
fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::none()
        .fill(SURFACE)
        .rounding(Rounding::same(RADIUS))
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::same(SP_MD + SP_XS))
        .show(ui, |ui| { ui.set_width(ui.available_width()); add_contents(ui); });
}
```

Taking `impl FnOnce(&mut egui::Ui)` lets every section compose its own content
inside a consistently styled surface. The result view iterates
`compression.placements()` and renders each placement uniformly, with a per card
**Copy** button that writes to the system clipboard via
`ui.output_mut(|o| o.copied_text = ...)`.

## Borrow discipline in a frame

There is a small but important ordering rule inside `update`. We `poll_result`
first (which may mutate `self.state`), then draw. The `inputs` section takes
`&mut self` because it edits the text buffers and may call `submit`, which sets
`state` and `rx`. The `output` section then matches on `&self.state` (a shared
borrow) and only reads. Because submit happens in an earlier statement than the
output match, there is no conflicting borrow, and a click shows `Loading`
immediately in the same frame.

## Rust primitives used in this module

- **trait impl** (`impl eframe::App`): plugs the type into the event loop.
- **enum sum type** (`UiState`): the explicit, exhaustive state machine.
- **struct owning state** (`CompressorApp`): runtime, buffers, state, receiver.
- **`mpsc` channel** (`std::sync::mpsc`): worker to UI handoff, non blocking.
- **tokio runtime + `spawn`**: runs the async engine call off the UI thread.
- **`Clone` on `egui::Context`**: a clone wakes the UI when the result is ready.
- **closures** (`impl FnOnce(&mut Ui)`): the composable `card` helper.
- **design tokens** (`const` colors, spacing, type scale, radius): one source of
  truth for the look.
