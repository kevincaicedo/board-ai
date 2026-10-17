# BoardAI: a voice-first harness for understanding

> Status: pre-MVP · Last updated: 2026-10-15 · Open source (Apache-2.0)
>
> **Design (source of truth for every screen):** [BoardAI design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ) · **Architecture:** [HLD](architecture/HLD.md) · [LLD](architecture/LLD.md) · **Decisions:** [DECISIONS.md](DECISIONS.md) · **Plan:** [PLAN.md](PLAN.md)

## TL;DR

BoardAI is a personal AI you **talk to** in order to understand things, starting with software. It explains the way a senior engineer does at a whiteboard. It speaks while it **draws hand-drawn diagrams in real time**, shows **code snippets that highlight in sync with what it's saying**, and lets you **interrupt at any moment** ("wait, wait — what's that?"). It answers, then picks up exactly where it stopped. Everything you explore feeds a **personal brain** of notes, a knowledge graph of past conversations, and tasks and roadmaps, so each session starts where the last one ended.

It runs on top of existing coding harnesses (Claude Code, pi), so it can read real codebases. It is built for **understanding first**: when you ask for a deep review or a change, it hands a precise brief to the harness.

The MVP depends on three things:

1. **The voice feels human.** Latency is low, the pace is natural, and interruptions are expected, not treated as errors.
2. **Drawing is synced to speech.** The diagram grows *while* it's being explained, stroke by stroke.
3. **The UI is almost nothing.** An orb, a canvas, and surfaces that appear only when they help.

---

## Design

The [design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ) is the source of truth for how BoardAI looks and moves. It has three pages:

- **Prototype:** the auto-playing demo (frames 1–7), the library, the brain, and the design system (tokens, orb states, canvas primitives, surfaces).
- **More examples:** sessions on how a Claude agent works, SSD flash memory, and recalling a past session; the brain's memories, knowledge graph and projects; use cases.
- **Landing page.**

Keeping design and code in sync:

- A UI change starts on the canvas, or lands there in the same change. When the canvas and the code disagree, the canvas wins until one of them is updated.
- §8 records the design principles. Exact values (colors, sizes, timings) come from the canvas's design system page.
- The canvas is private to the owner's claude.ai account. Outside contributors work from the tokens and screenshots exported into the repo.

---

## 1. Problem

- **Understanding is the bottleneck, and the tools are built for doing.** Coding agents optimize for producing diffs. Chat UIs return walls of text. Neither helps you build a mental model of a kernel subsystem, a consensus protocol or an unfamiliar repo.
- **The best format for learning structure doesn't exist in software.** That format is a person at a whiteboard who draws while talking. You interrupt, and they back up, zoom in and redraw.
- **Text chat is the wrong shape for this.** You can't interrupt a streaming answer. Structure gets described instead of shown. Each conversation is forgotten, so "Raft on Monday" and "etcd on Thursday" never connect.
- **The adjacent tools each cover only part of it.** Voice assistants can talk but not show. Diagram tools can show but not talk or teach. Neither knows your code or remembers what you've already learned.

## 2. Vision and positioning

**One-liner:** *A whiteboard that talks back, and remembers.*

**Vision:** A second brain you converse with in real time. It explains, draws, simulates and remembers, so your understanding compounds over time. It starts with software and can extend to any complex domain later.

| BoardAI is | BoardAI is not |
|---|---|
| A voice-first partner for understanding systems and code | A coding agent that writes code for you (it can hand off to one) |
| Live, interruptible, hand-drawn explanations | A diagram generator that hands you a finished image |
| A memory of everything you've explored | A chat log |
| A calm, mostly empty screen | A dashboard, an IDE, or a chat window |

## 3. Who it's for

| Persona | Job to be done | Example topics |
|---|---|---|
| **Self-directed systems learner** (primary user; we use it ourselves first) | "Make me *really* understand X, at my pace, and remember what I learned." | Linux VFS, Raft, B-trees, CPU caches, AWS networking, memory allocators |
| **Engineer onboarding onto a codebase** | "Walk me through this repo. How does a request actually flow?" | A new job, an open-source contribution |
| **Architect thinking out loud** | "Talk this design through with me and show me the tradeoffs." | Kafka vs SQS, cache strategy, a sharding plan |
| *Later:* interview prep, students, learners outside software | | Mock system design interviews |

## 4. Product principles

1. **Voice is the interface and the screen is the whiteboard.** Nothing is typed in the MVP. The screen shows what words can't.
2. **Show structure instead of describing it.** If an explanation has boxes and arrows, BoardAI draws them.
3. **Interruption is a feature.** Being cut off mid-sentence is the normal path. It gets designed and tested like any other.
4. **Human pace.** Teaching tempo, with pauses after key points and check-ins at natural boundaries. You control the speed.
5. **Calm technology.** Nothing stays on screen unless it's helping you understand *right now*. Surfaces appear, do their job, and dissolve.
6. **Grounded.** When BoardAI talks about your code, every claim comes from a file it actually read, with a `file:line` reference. "I haven't looked at that yet" is an acceptable answer.
7. **It remembers you.** Sessions connect to each other. BoardAI knows what you've covered and where you struggled.
8. **Local-first, and yours.** Notes are Markdown, diagrams are open JSON, and the index is SQLite on your machine.

---

## 5. The core experience

This is the moment the product has to nail. It doubles as the demo.

> **You:** "Walk me through how a request reaches our API, from the browser to Postgres."
>
> The orb condenses into a glowing nib and glides onto the canvas.
> **BoardAI:** "Sure. It starts in the browser…" A hand-drawn *Client* box sketches itself. "…hits the CDN at the edge…" An arrow draws out and a *CDN* box appears. "…then the load balancer spreads it across three API pods. Before touching Postgres, the service checks Re—"
>
> **You:** "Wait — why Redis first?"
>
> The voice trails off and the nib freezes partway through drawing the *Redis* box. The main diagram blurs into the background and a detour card comes forward.
> **BoardAI:** "That's the cache-aside pattern. Reads go to Redis first because…" It sketches a small hit/miss diagram, then opens `internal/user/service.go`. Lines highlight as it talks, and a pink dot travels along the miss path.
>
> **You:** "Got it. Note that, and add a task to check our TTLs."
>
> A note chip and a task chip float to the right edge of the screen.
> **BoardAI:** "Noted. So, back to the request. On a miss, it goes to Postgres…" The detour dissolves, the nib finishes the Redis box from exactly where it stopped, and the explanation continues.

A week later you ask "Where were we on the API stuff?" The diagram comes back, BoardAI recaps it in two sentences, and mentions that the TTL task is still open.

---

## 6. Interaction model

### 6.1 Turn-taking

- **The mic is open by default**, with echo cancellation. **Push-to-talk** (hold `Space`) is an option for noisy places. Headphones are recommended.
- **End of turn is detected from voice activity plus semantic completeness.** A pause after "so the leader…" doesn't end the turn.
- **BoardAI can start speaking before it has finished thinking, but only with real content.** For example: "Let me open the router… OK, there are three entry points." It never uses empty filler.
- **Long harness work is visible.** While the coding harness reads files, BoardAI says what it's looking at. The canvas shows it peripherally, with file names drifting faintly at the edge.

### 6.2 Interruptions

BoardAI classifies every utterance you make while it's speaking:

| Kind | Examples | What BoardAI does |
|---|---|---|
| **Backchannel** | "mm-hm", "right", "ok", "yeah" | Keeps going. The orb gives a small acknowledging pulse. |
| **Clarify** | "wait, what's an inode?", "why?" | Stops, opens a **detour**, answers, then resumes. |
| **Point** | "what's *this*?" while the pointer is on a node or line | Same as clarify, scoped to the element you're pointing at. |
| **Redirect** | "skip that, show me the code" | Drops the current step and replans. |
| **Correct** | "no, we use Kafka, not SQS" | Strikes through the old label and rewrites it on the canvas, then updates memory. |
| **Control** | "slower", "stop", "go back", "repeat that", "zoom out" | Applies the change immediately. |
| **Capture** | "note that", "remember this", "add a task…" | Writes to the brain, confirms in three words or fewer, and continues. |

**Duck first, decide second.** When you start speaking, BoardAI's volume drops within about 100 ms, the way a person trails off. It stops completely only once your utterance is classified as a real interruption. If it was a backchannel, the volume comes back up with no awkward full stop.

### 6.3 Thread, detours and resume

- **Thread.** Before any non-trivial explanation, BoardAI makes a short plan. The thread shows it as a thin progress line with step dots: Client → Edge → LB → Service → Cache → DB.
- **Detours.** An interruption pushes a **detour** onto a stack. Detours can nest ("wait, and what's a TTL?"). They appear as small branches off the thread.
- **Resume.** Finishing a detour pops it off the stack. BoardAI says a bridge sentence ("So, back to the load balancer") and continues from the exact word and stroke where it stopped.
- **BoardAI only assumes you heard what it actually said before the cut.** Unspoken text is dropped from its context. Strokes that weren't drawn yet are replayed on resume.

### 6.4 Pointing

Voice plus pointer is the only non-voice input in the MVP. Hover over or click a node, an arrow or a line of code while saying "this", "here" or "that one", and BoardAI resolves the reference from the pointer target. BoardAI also points back: its nib circles, underlines and draws arrows to whatever it's talking about.

### 6.5 Pace and depth

You control these by speaking naturally. There's no command syntax to learn. Some examples:

"slower" · "faster" · "pause" · "go deeper" · "simpler" · "skip" · "back up" · "zoom out" · "show me the code" · "compare it with…" · "simulate it" · "recap"

### 6.6 Voice and personality

BoardAI sounds like a calm senior engineer at a whiteboard:

- It uses short, concrete sentences.
- It checks in at natural boundaries ("make sense so far?").
- It admits uncertainty.
- It never keeps lecturing past an interruption.
- It uses analogies sparingly.

The MVP has one default voice, warm and unhurried. A choice of voices comes later.

---

## 7. Features

### 7.1 Live canvas with human-like drawing

BoardAI draws on an infinite canvas in an Excalidraw-like hand-drawn style. Drawing happens stroke by stroke, in sync with its voice.

**What makes the drawing feel human:**

- **Shapes are drawn as strokes, not faded in.** Speed varies the way a hand's does: faster on straight lines, slower into corners, with a slight overshoot.
- **Drawing follows a human order.** First the outline, then the label written letter by letter, then the connector from source to target, then the arrowhead, then the hachure fill.
- **Drawing never gets ahead of the narration.** Stroke speed adapts to the speech.
- **Corrections are visible.** BoardAI strikes through the old label and rewrites it instead of silently swapping it. Saying "tidy up" cleans the board later.
- **Annotations are orange.** BoardAI uses circles, underlines, brackets, margin notes and pointing arrows.
- **BoardAI directs the camera.** In follow mode it pans and zooms to what it's explaining. Grab the canvas and you take control; say "follow me again" to hand it back.
- **Depth of field shows focus.** The element under discussion is sharp and everything else softens slightly. A detour blurs the main board into the background.

**Diagram types in the MVP:**

- Architecture and flow diagrams: boxes, cylinders, queues, actors, groups
- Sequence diagrams
- State machines
- Data structure and memory layouts: arrays, pages, trees, hash tables (like reference image 1)
- Call graphs
- **Detailed system diagrams**, as dense as a hand-made Excalidraw map: containers nested several levels deep (cloud › VPC › subnet › node group), color-coded categories with a legend, notes and numbered steps written inside the diagram, structs and memory layouts with their fields, labelled arrows, dashed boundaries. A single board can hold a hundred or more elements; BoardAI still draws them in a human order, a part at a time, as it explains.

**Simple or detailed, and deeper on request.** BoardAI fits the drawing to the question: a quick sketch for "what's a cache?", a full map for "walk me through inf-alloc". Saying "more detail on the arena" drills into that part: the camera moves into it and a detail layer opens, anchored to it, with its internals. Each level can go deeper again, and "back out" returns. Large diagrams are planned whole before the first stroke, so nothing already drawn ever moves.

**Later:** timelines, layered stacks (OSI, kernel layers) and plots.

### 7.2 Dynamic UI surfaces

BoardAI chooses **what** to show and the design system decides **how and where**. BoardAI composes from a fixed library of building blocks, which keeps generated UI looking good:

| Surface | Purpose |
|---|---|
| **Canvas** | The whiteboard. It is always the main stage. |
| **Code card** | A snippet with a path chip (`service.go · L42–58`). Lines highlight in sync with speech, and pen annotations can be drawn on top of the code. |
| **Trace** | A simulated code path. A dot travels along the diagram's edges while the code card's highlight moves in lockstep, with a small call stack alongside. |
| **Simulation** | Animated state on a diagram: nodes fail, messages fly between them, a leader gets elected. |
| **Compare** | A tradeoff matrix of options × criteria, with hand-written ✓, ✗ and "depends". |
| **Doc card** | An excerpt pulled from docs, a paper, a README or a man page, with the relevant passage highlighted. |
| **Note and task chips** | Captured items that fly to the brain edge. |
| **Recap** | A summary of 3–5 bullets, shown at the end or on request. |
| **Thread** | Step progress, including detours. |
| **Captions** | A live, faint transcript under the orb. |

Surfaces follow three rules:

- No more than two surfaces sit beside the canvas at once.
- You can dismiss any surface by voice ("close that").
- Surfaces leave when the topic moves on.

### 7.3 Code understanding (on top of coding harnesses)

- **Understanding code goes through a coding harness.** Questions about a shared codebase, a crate, a module or a whole repo, go to Claude Code (headless CLI) or pi (RPC mode) as research. The harness reads the code and returns a brief: the components and how they relate, the main flows with `file:line`, and a diagram at the depth you asked for. BoardAI draws it and explains it step by step, and asks the harness again when you want to go deeper. While it reads, BoardAI says what it is looking at.
- **Quick looks stay native.** Showing a file's lines in a code card, and checking every `file:line` a brief cites before its chip appears, are read-only native reads, in milliseconds.
- **Reviews and changes go to the harness too.** BoardAI hands it a precise brief and narrates the progress while it runs.
  - Research and reviews are read-only.
  - A change runs only after you confirm it out loud, and always on a new git branch.
- **Repo tours.** Saying "Walk me through this repo" makes BoardAI draw a module map on the canvas, then take you through the main paths one at a time.
- **Source chips.** Every claim about code carries a `file:line` chip. Clicking it opens the code card.
- **Trace mode.** Ask "What happens when I call `POST /orders`?" and BoardAI steps through the call across files, with the diagram and the code moving together.
- **Pull.** You can ask BoardAI to bring up information from the repo, from memory, or from the web and docs:
  - "Pull up how we configure the retry policy."
  - "Show me the part of the Raft paper on log matching."
  - "What did we say about this last week?"
- **Speech recognition knows your code.** Function, type and service names from the open repo feed the speech-to-text vocabulary, so `getUserByID` isn't heard as "get user by eye dee".

### 7.4 Memory: the personal brain

- **Auto-notes.** BoardAI takes notes quietly during the session. At the end it writes a **session digest** with a summary, the key diagrams, open questions and any captured tasks.
- **Voice capture.** You can say "note that", "remember that our cluster uses…" or "add a task…".
- **Knowledge graph.** The graph links concepts, systems, files, projects, sessions and notes. The relationships are `explains`, `part_of`, `depends_on`, `mentioned_in` and `struggled_with`.
- **Recall.** Asking "What did we cover on consensus?" or "Show me the diagram from the Redis session" brings back the canvas with a two-sentence recap.
- **Learner model.** BoardAI tracks what you've covered, what you asked about repeatedly, and your preferred pace and depth. Interruptions are a useful signal of confusion. BoardAI uses this to skip what you already know and to revisit topics where you were shaky.
- **Plain files.** Notes are Markdown and diagrams are `.excalidraw`-compatible JSON, stored in a folder you own.

**What BoardAI remembers on its own.** The balance: keep what is useful and visible, ask before keeping what was inferred, and never keep what is risky.

| Kind | Example | Default |
|---|---|---|
| The session itself | Transcript, board, captured items | Kept automatically, on your machine |
| Session digest | Summary, diagrams, open questions | Written automatically at the end |
| Learning signals | Topics covered, "interrupted 3 times on log matching", pace changes you asked for | Kept automatically; visible and editable in Memory |
| What you state about yourself or your systems | "I prefer Go examples", "we use Kafka" | Kept automatically, in your words |
| What BoardAI infers | "Likes a slower pace", "service X uses cache-aside" | Proposed in the digest; kept only after you say yes |
| Notes and tasks | "Note that…", "add a task…" | Only when you ask |
| Your raw voice audio | | Never stored; only the transcript |
| Secrets | API keys, tokens, passwords | Never stored; redacted before anything is written |

Two controls cover the rest. "Forget that" removes the last memory. "Don't remember this session" leaves no trace of the session at all.

### 7.5 Board: tasks, projects and roadmap

- **Tasks are captured by voice.** Each one links to the session, diagram node or `file:line` it came from.
- **Projects group things.** A project ("the harness project") collects tasks, notes and sessions.
- **Views appear on request.** **Now** is a short list, **Board** shows columns, and **Roadmap** is a timeline. You call them up by voice ("what's next on the harness?") and they show as a surface, never as a permanent screen.
- **MVP scope:** tasks, projects and the Now list. The Board and Roadmap views come later.

### 7.6 Sessions and replay

- **Sessions replay like a film.** Every session is an event log of voice, strokes and surfaces. You can scrub the timeline, which has chapters at each thread step and shows detours as branches.
- **Library.** Sessions appear as large rounded cards. Each has soft abstract cover art (a blurred pastel render of the session's diagram) and the play, mute and progress controls from the references.
- **Export.** Diagrams export as PNG, SVG or `.excalidraw`, notes as Markdown, plus the transcript.

---

### 7.7 Sources: what you want to understand

You share what you want to learn from, and BoardAI keeps it at hand.

- **What you can share:** a codebase, a crate or module folder, a single file, a Markdown or text document, or a PDF (a paper, a spec, a book chapter).
- **How:** drop it on the window, add it from the Sources tray (the macOS chooser), or say it ("use the inf-alloc crate", resolved inside the code folders you allowed).
- **For this session or for a project.** A source is for this session unless you keep it in the project ("keep this in the infinitydb project"). A project's sources are there in every session of that project, with what BoardAI already learned about them, so a second question about the same crate is fast.
- **Code** is always understood through the coding harness (§7.3).
- **Documents** are read without losing what matters:
  - Text, headings, lists and tables are extracted on this Mac.
  - Figures and diagrams are read too, by the talker model when it accepts files and images, or by the coding harness when the document is long or the question is deep.
  - Each document gets a digest: its sections, its figures described as diagrams BoardAI can redraw, its tables, and its key claims with page numbers.
  - A Doc card shows the passage or figure being discussed, with a `p. 5 · Figure 3` chip.
- **Private by default.** Sources are read on this Mac. Their text goes only to the providers you configured (the talker, the harness), and the tray says which one.

## 8. Design language

> The [design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ) is authoritative. This section records the principles behind it; when the two disagree, the canvas wins and this section is updated.

### 8.1 Mood

Porcelain, frosted glass and soft light. The look is extremely minimal and futuristic but calm: *an Apple product film meets Excalidraw*. Most of the screen is white, and the orb is the only element with lively color. The hand-drawn canvas brings warmth and imperfection into an otherwise precise, silent interface.

| Ref | Image | What we take from it |
|---|---|---|
| 1 | Hand-drawn memory-region diagram | The canvas style: rough strokes, pastel hachure fills, handwritten type, orange annotation arrows |
| 2, 3, 6 | Soft porcelain and fabric 3D renders | Material and light, heavy depth of field, session cover art, ambient backdrops |
| 3, 6 | Round white controls with a pink glyph, thin pink progress bar | The control style, the single accent color, the replay scrubber and the thread bar |
| 6 | Concentric rings of dots | The listening visualization: the dot field ripples with your voice |
| 4, 5 | Iridescent orb shading from blush to violet | BoardAI's presence. Every state derives from it. |

### 8.2 Color tokens (light theme, primary)

| Token | Value | Use |
|---|---|---|
| `bg` | `#FFFFFF` | Canvas and app background |
| `bg-soft` | `#FAFAFB` | Library and brain |
| `grey-100` / `grey-200` / `grey-300` | `#EEEFF2` / `#D9DBE0` / `#C5C8CF` | Porcelain greys, dividers, tracks |
| `ink` | `#1D1D1F` | Primary text and sketch strokes |
| `ink-2` | `#6E6E73` | Secondary text and captions |
| `ink-3` | `#A1A1A6` | Tertiary text, older captions, ghost text |
| `signal` | `#D9548B` | The one accent: live state, active state, progress, the trace dot |
| `orb-1…4` | `#F6CFE0` → `#E58BB4` → `#B565D8` → `#DCD3F7` | Orb gradient only |
| `sketch-lilac` / `sky` / `mint` / `blush` | `#D9C8F5` / `#C4DEF6` / `#CFEFD9` / `#F7D3E0` | Hachure fills on the canvas |
| `annotate` | `#E8913A` | Pen annotations |
| `glass` | `rgba(255,255,255,.62)` with a 32px backdrop blur | Surfaces |

Gradients appear only in the orb and in cover art. A dark theme ("obsidian") comes later.

### 8.3 Typography

| Role | Typeface | Notes |
|---|---|---|
| UI and display | Inter Display or Geist | Light (300) at large sizes, regular at small sizes, generous whitespace |
| Canvas | Excalifont or Virgil (handwritten) | Everything BoardAI writes on the board |
| Code | Geist Mono or JetBrains Mono | 14px, line numbers in `ink-3` |
| Captions | The UI typeface at 17px | The current line is `ink-2`. Older lines fade to `ink-3`, then disappear. |

### 8.4 Shape, depth and material

- **Radii.** Cards use 28px, chips 16px, and buttons are fully round. Buttons are 56px white circles with a soft shadow and a `signal` glyph, taken straight from the references.
- **No hard borders.** Elements are separated by light, blur and shadow (`0 24px 64px rgba(20,20,40,.08)`).
- **Surfaces are frosted glass** floating above the canvas, and they cast soft shadows on the drawing.
- **Depth of field directs attention.** Out-of-focus content gets a 2–6px blur and 60% opacity.

### 8.5 Motion

- **Everything flows and nothing snaps.** The orb and surfaces move on springs. Everything else uses ease-out (`cubic-bezier(.22,1,.36,1)`).
- **Durations:** 160 ms for micro-interactions, 420 ms as the standard, 900 ms for scene changes.
- **Surfaces** enter by rising 12px while coming into focus from a blur, and leave by blurring out.
- **Strokes** animate along their path length using a human velocity curve. Handwriting appears letter by letter.
- **Reduced motion is respected.** Strokes appear in segments, the camera doesn't move, and the orb becomes a static gradient.

### 8.6 The orb, BoardAI's presence

The orb is the only element that is always on screen.

**Signature moment: the orb becomes the pen.** When BoardAI starts drawing, the orb condenses into a small luminous nib that glides onto the canvas and draws. When it's done, the nib floats back and swells into the orb again.

| State | The orb | Elsewhere on screen |
|---|---|---|
| Idle | Centered, breathing slowly on a 6 s cycle, colors drifting | A faint suggestion below it ("Pick up Raft log compaction?") |
| Listening | Swells by about 8%, with ripples on its surface that follow your voice. A concentric dot field radiates behind it. | Your words appear as captions in `ink-3` |
| Thinking | The swirl inside it tightens and rotates | Ghost file names drift at the edge if the harness is working |
| Speaking | Pulses gently with the voice | Captions in `ink-2` |
| Drawing | Condensed into a 10–12px nib on the canvas, with a small echo of the orb docked at bottom center | Strokes flow from the nib |
| Interrupted | Contracts quickly (120 ms), leans toward your pointer, and its colors cool slightly | The stroke freezes partway along its path. The main board blurs back if a detour opens. |
| Muted | Desaturated and still | The mic button shows a slashed glyph in `signal` |

### 8.7 Layout

There is no chrome: no sidebar, no top bar, no text box.

- **Stage.** The canvas fills the whole window.
- **Orb.** Centered while conversing, docked at bottom center while the canvas is active, and on the canvas as the nib while drawing.
- **Thread.** A thin line with step dots at top center, visible only during explanations.
- **Surfaces.** Float in the right third of the screen, no more than two at a time.
- **Captions.** At bottom center, above the docked orb.
- **Brain edge.** A hairline glow along the right edge where notes and tasks fly in. Saying "show my notes" pulls it open.
- **Controls.** A single round mic button at bottom right. The replay scrubber appears along the bottom on hover.

### 8.8 Key screens

The design canvas has every key screen as a frame: 1 Idle, 2 Listening, 3 Explaining and drawing, 4 Interrupted → detour, 5 Code walkthrough with trace, 6 Capture, 7 Resume, 8 Library and 9 Brain. The examples page adds the memory, knowledge graph and projects screens.

### 8.9 Accessibility

- **Captions are on by default.** They also help with technical terms and in noisy places.
- **Reduced-motion mode** works as described in §8.5.
- **Full keyboard control:** hold `Space` for push-to-talk, `Esc` to stop or interrupt, `M` to mute.
- **Contrast.** Ink on white is at least 7:1. Pastel fills never carry meaning on their own; there is always a label.
- **Voice-only input excludes some people.** A text box arrives in Phase 2.

---

## 9. Architecture

The architecture lives in [architecture/HLD.md](architecture/HLD.md) (high level, including the latency budget) and [architecture/LLD.md](architecture/LLD.md) (low level). In one sentence: inside a Tauri desktop app, a pure Rust core decides, a Rust runtime does all the I/O, and a thin web UI shows the result.

---

## 10. MVP scope

**In scope**

- A desktop app for macOS 14+ on Apple silicon, with a light theme. Open source, bring your own keys.
- **Providers, all swappable:**
  - LLM: Claude, OpenRouter (any listed model; measured presets include gpt-oss-120b, Qwen3.8, DeepSeek V4.1 Flash, Claude Haiku 5.5, Gemini 3.8 Flash and GPT-6 Luna), or a local model through Ollama
  - Speech-to-text: local Whisper by default
  - Text-to-speech: a local voice model (Kokoro) by default, or ElevenLabs
  - Coding harness: Claude Code (headless CLI) or pi
- **Voice loop:**
  - Open mic by default, with push-to-talk as an option
  - Barge-in using duck-then-decide, with backchannel handling
  - Captions
- **The orb**, with every state and the orb-to-nib transition.
- **Live canvas:**
  - Architecture, flow, sequence and data-structure diagrams, from quick sketches to detailed nested system maps, with drill-down into any part
  - Human-like strokes synced to speech
  - Strike-and-rewrite corrections and annotations
  - Camera follow and depth of field
- **Explanations:** the thread, nested detours and exact resume.
- **Pointing:** hover or click while saying "this".
- **Sources (§7.7):** codebases, crates, modules, files, Markdown and PDF documents, shared for a session or kept in a project, with document digests that keep figures.
- **Code grounding:**
  - Code understanding through the coding harness (research briefs with diagrams), native read-only looks for code cards and citation checks, and the harness for reviews and confirmed changes
  - Module map
  - Code cards with synced highlights and source chips
  - Trace mode
- **The Compare surface.**
- **Memory:** notes and tasks by voice, an automatic session digest, the memory policy in §7.4, and recall of past sessions.
- **Stretch:** the session library and replay.

**Out of scope for now**

- Text input
- Windows, Linux and mobile
- Multiplayer
- Editing code inside BoardAI (changes go through the coding harness)
- Visualizing the knowledge graph
- Board and roadmap views
- Simulations other than trace
- Domains outside software
- A hosted version
- Dark theme

## 11. Roadmap

| Phase | What ships | Exit criteria |
|---|---|---|
| **0 · The magic trick** | The orb, the voice loop with barge-in, and hand-drawn architecture diagrams synced to speech, on general topics. No repo and no memory yet. | Ten of your own study sessions (Linux VFS, Raft, B-trees…) where interrupting feels natural and you'd rather use BoardAI than read. |
| **1 · MVP** | Repo grounding, code cards, trace, detours and resume, Compare, notes and tasks, session memory and recall, library and replay. | You get to know an unfamiliar open-source repo using only BoardAI, and 5 outside engineers use it every week for a month. |
| **2 · Personal brain** | A visual view of the knowledge graph ("constellation"), Board and Roadmap views, a learner model that revisits shaky topics, simulations, an MCP server that exposes the brain, a text box, a dark theme. | Weekly retention, and sessions that build on earlier ones. |
| **3 · Beyond software** | Other domains (math, physics, business), a mock system design interview mode (you draw and explain while BoardAI interviews), shared sessions, a mobile companion for listening and review. | |
| **4 · Hosted** | A web version: the same core running in the cloud, managed models by subscription, and sync across devices. It starts only after the MVP loop is validated. | People pay for it instead of managing their own keys. |

## 12. Success metrics

- **How it feels** (instrumented):
  - Voice-to-voice latency at p50 and p90
  - How fast BoardAI ducks and stops when barged in on
  - Backchannels mistakenly treated as interruptions: under 5%
  - Real interruptions missed: under 2%
  - Drift between speech and strokes
- **Value:**
  - Sessions per active user per week
  - Median session length
  - **Continuity rate:** the share of sessions picked up again later
  - The share of sessions that produce a note or task
  - Understanding rated by the user before and after (1–5), on a sample of sessions
- **Grounding:**
  - The share of code claims that carry a valid source chip
  - User corrections per session (fewer means better grounding)

## 13. Risks and mitigations

| Risk | Why it matters | Mitigation |
|---|---|---|
| Latency adds up across STT, LLM, TTS and layout | The conversation stops feeling human | Stream every stage, use a fast tier for the talker, cache prompts, start drawing on the first word, prefetch research |
| LLM-generated diagrams look messy | It breaks the magic | Ops carry meaning only and the layout engine owns geometry, with templates for each diagram type and stable incremental layout |
| False barge-ins from echo, coughs or "mm-hm" | Explanations become choppy | Echo cancellation, recommending headphones, duck-then-decide, a backchannel classifier |
| Hallucinated explanations of code | It destroys trust | Claims come only from the researcher, with a source chip on each, and an honest "I haven't read that yet" |
| Friction from voice-only input (spelling identifiers, noisy rooms) | Frustration | Pointing, captions, push-to-talk, STT vocabulary built from the repo, a text box in Phase 2 |
| Scope creep: a "personal brain" never ends | The MVP never ships | The MVP is the magic trick plus code grounding. Brain features only emerge as byproducts of sessions. |
| Cost per hour of voice | Pricing | A fast tier, prompt caching, local VAD and later local STT |
| Big labs add voice and canvas modes | Being commoditized | Depth for engineers, grounding in *your* code, drawing built around interruptions, local-first memory |

## 14. Landscape (adjacent products, not equivalents)

| Category | Examples | What BoardAI adds |
|---|---|---|
| General voice assistants | ChatGPT Voice, Gemini Live | Diagrams drawn in sync with speech, and knowledge of your repo |
| AI explainers | NotebookLM audio and video overviews | A live conversation you can interrupt, instead of pre-rendered media |
| AI diagramming | Excalidraw and tldraw AI features, Eraser, Napkin | A narrating teacher; drawing that happens live |
| Coding agents | Claude Code, Cursor, pi | Built for understanding rather than changing code (and it uses them underneath) |
| Notes and PKM | Obsidian, Notion, Tana | Memory captured automatically from conversation |

**The wedge** is the combination: live, interruptible voice plus drawing, grounded in your code, with memory. It's for engineers first.

## 15. Decisions

The open questions from the first draft are answered. [DECISIONS.md](DECISIONS.md) records each decision with its reason.

| Question | Decision |
|---|---|
| Name | **BoardAI.** "openboard" was also proposed, but OpenBoard is already an established open-source whiteboard app. |
| Voice pipeline | Cascaded (STT → LLM → TTS) for the MVP |
| Desktop shell | Tauri |
| Canvas | A custom renderer with an Excalidraw-compatible format |
| Mic | Both modes. Open mic is the default; push-to-talk is an option. |
| Voice | One default voice for now; a choice of voices later |
| Business model | Open source and bring your own keys, with a choice of providers (§10). A hosted version with managed models and sync comes after the MVP is validated (Phase 4). |
| Memory | The balance in §7.4: automatic for the session record and learning signals, proposed for anything inferred, only on request for notes and tasks, never for raw audio or secrets |

**Still open**

1. **License.** Apache-2.0 for now. AGPL-3.0 would stop others from running a closed hosted copy, at the cost of fewer corporate contributors. It's easy to change before the first outside contribution and hard after.

---

## Appendix A: Glossary

| Term | Meaning |
|---|---|
| **Orb** | BoardAI's visual presence and the only element always on screen |
| **Nib** | The orb condensed into a pen while drawing |
| **Canvas** | The infinite hand-drawn whiteboard |
| **Surface** | A dynamic UI card (code, compare, doc, recap…) that appears and dissolves |
| **Thread** | The plan for an explanation, shown as step progress |
| **Detour** | A branch opened by an interruption. Detours nest and resume back into the thread. |
| **Trace** | A simulated code path, with a dot on the diagram moving in lockstep with highlighted code |
| **Brain** | Memory: notes, the knowledge graph, tasks, projects |
| **Brain edge** | The glowing right edge where captured items fly in |
| **Talker / Researcher** | The fast conversational model / the coding harness that reads code |
| **Narration script** | The talker's output: speech with drawing ops interleaved |
