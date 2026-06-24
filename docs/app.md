# app

The egui interface, the Pressroom theme, and the async to UI bridge. This is
where the typed pieces become something a person uses: enter a key, pick an
engine, compress, and see the result graded and explained.

Source: [`src/app.rs`](../src/app.rs)

## The shape of an egui app

egui is immediate mode: there is no retained widget tree. Every frame, eframe
calls `update` and you describe the entire interface from current state. Our type
plugs in with a trait impl:

```rust
impl eframe::App for CompressorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) { ... }
}
```

Because the whole UI is redrawn from state each frame, the cleanest model is an
explicit state machine.

## The state machine

```rust
enum UiState {
    Idle,
    Loading,
    Result(Box<ResultBundle>),
    Error(String),
}
```

An enum sum type makes the four screens mutually exclusive. `ResultBundle` is
boxed because it is much larger than the other variants, which keeps the enum
small. The bundle is the graded result:

```rust
struct ResultBundle {
    output: EngineOutput,            // compression + optional trace
    promise_ground: GroundednessReport,
    readability: Readability,
    flags: Vec<Flag>,
}
```

The app struct owns the runtime, the selected engine, the input buffers, the
state, and the receiver:

```rust
pub struct CompressorApp {
    runtime: tokio::runtime::Runtime,
    engine: EngineKind,
    product_text: String,
    audience_text: String,
    state: UiState,
    rx: Option<Receiver<Result<ResultBundle, EngineError>>>,
}
```

The engine defaults to `Heuristic`, because it is instant, offline, and needs no
key, which is the best first impression and showcases the deterministic core.

## The async to UI bridge

The bridge keeps the interface responsive while a model call is in flight.

**1. A tokio runtime owned by the app**, built once in `new` with `enable_all`
so reqwest has the IO and timer drivers it needs.

**2. An `mpsc` channel.** On submit, the app validates the text into domain
newtypes (a bad input goes straight to `Error` without touching anything else),
creates a channel, keeps the receiver, and spawns the selected engine:

```rust
let kind = self.engine;
let api_key = self.effective_key();   // the window field, or None (env fallback)
let grounded_src = format!("{}\n{}", product.as_str(), audience.as_str());
let ctx = ctx.clone();

self.runtime.spawn(async move {
    let result = engine::run(kind, api_key, product, audience).await.map(|output| {
        // The scorer sits outside the engine and grades its output.
        let grounded = analysis::Grounded::build(&grounded_src);
        let promise_ground = grounded.report(output.compression.promise.as_str());
        let readability = analysis::readability(&output.compression.meta_primary_text);
        let flags = analysis::constraint_flags(&output.compression);
        ResultBundle { output, promise_ground, readability, flags }
    });
    let _ = tx.send(result);
    ctx.request_repaint();
});
```

Two things to note. First, `engine::run` returns a boxed future, so the same
`spawn` works for any engine the user picked. Second, the grounded source is
captured before the inputs move into the engine, and the grading runs in the task
after the engine returns, which keeps the grading independent of the engine and
off the UI thread. A clone of the egui context wakes the UI exactly when the
result is ready.

**3. A non blocking poll once per frame.** `poll_result` calls `try_recv`, which
returns immediately whether or not a value is ready, and advances the state
machine when one arrives.

## The engine picker

The inputs card opens with a segmented control of chips, one per engine, drawn
with a small helper that fills the selected chip with the accent color:

```rust
for kind in ENGINES {
    if engine_chip(ui, kind.label(), self.engine == kind).clicked() {
        self.engine = kind;
    }
}
```

Below it, `self.engine.tagline()` explains the tradeoff, and when the selected
engine needs the key, a quiet "needs ANTHROPIC_API_KEY" hint sits next to the
Compress button.

## Rendering a graded result

`render_result` draws, in order:

- the **core need** card,
- the **promise** card, with a groundedness badge ("94% grounded") colored by
  band on the right, and, if any terms are unsupported, a line naming them,
- a **quality checks** card with readability and groundedness chips and either a
  green "no issues" line or the constraint flags,
- the **three placements**, rendered uniformly from `compression.placements()`,
  each with a Copy button and, for the Google headlines, a live `n/30` character
  count that turns red on overrun,
- and the **explainability panel**, when the engine left a trace.

The groundedness badge color comes from a small `ground_color` helper: green at
85 and above, amber at 70 to 84, red below.

## The explainability panel

This is the moat. When `output.trace` is present (the deterministic and hybrid
engines), a collapsing panel shows the engine's work:

- the core need source sentence and its TextRank score,
- the name of the winning promise template,
- every promise candidate, ranked, each with its groundedness, readability, and
  combined score, with the chosen one marked,
- and any audit notes (for hybrid, the groundedness audit of the model's promise).

No model backed tool can offer this, which is exactly why it is shown by default.

## The API key entry

The key bar sits on the ink ground above the wordmark: a masked single line field
(`TextEdit::password`), a show / hide toggle, a status indicator that reads
"KEY LOADED" or "NO KEY", and a "remember on this device" checkbox.

`effective_key` returns the trimmed field value, or `None` when it is blank; the
model engines then fall back to the `ANTHROPIC_API_KEY` environment variable, so
either source works. When "remember" is ticked, the key is persisted with the
small `keystore` module (std file IO only, no extra dependency) to
`%APPDATA%\Compressor\key.txt`, and loaded on startup. Unticking it deletes the
file. The path is plain text under the user profile, which the bar states
explicitly when the box is checked.

## The Pressroom theme

`install_theme`, called once at startup, defines the look from named design
tokens. The concept is a letterpress instrument: a deep warm ink ground, bone
paper cards, a single vermilion ink accent, and printed greens, ochres, and reds
for the quality signals. Text is dark ink on the paper cards and paper on the ink
ground, so `override_text_color` defaults to ink and on ink elements are colored
explicitly (an explicit `RichText` color always wins).

The character comes from custom painting with `egui::Painter`, not just colored
rectangles:

- the **wordmark** is spaced caps ("C O M P R E S S O R") for a pressed feel,
- a **vermilion rule** under the header is capped with printer's **registration
  marks** (a circle with a centered cross), drawn by `draw_regmark`,
- groundedness is an **ink coverage meter** (`ink_meter`): a paper track filled to
  the score in the band color, captioned "N% inked", because a grounded promise is
  a well inked one,
- the **Compress button** is an inked platen (vermilion fill, spaced caps),
- the engine picker chips are **ink stamps** (vermilion when selected),
- labels are monospace uppercase eyebrows, and cards have crisp printed corners.

There is a spacing scale and a type scale mapped onto egui's semantic text
styles. The whole look lives in one place, applied by cloning the `Style`,
mutating its `text_styles`, `visuals`, and `spacing`, and calling `set_style`.

## Rust primitives used in this module

- **trait impl** (`impl eframe::App`): plugs into the event loop.
- **enum sum types** (`UiState`, `EngineKind`): the state machine and engine
  choice.
- **`Box` in an enum variant**: keeps the large result off the small variants.
- **`mpsc` channel + `try_recv`**: non blocking worker to UI handoff.
- **tokio runtime + boxed futures**: run any selected engine uniformly.
- **`Clone` on `egui::Context`**: wake the UI when the result is ready.
- **closures** (`impl FnOnce(&mut Ui)`): the composable `card` helper.
- **`egui::Painter`**: custom drawing for the rule, registration marks, and meter.
- **std file IO** (`keystore`): remember the key on this device, no extra crate.
- **design tokens** (`const` colors, spacing, type scale): one source of truth.
