# BoardAI: the board for understanding

> Status: MVP built, in validation · Last updated: 2026-10-17 · Open source (Apache-2.0) · [github.com/kevincaicedo/board-ai](https://github.com/kevincaicedo/board-ai)
>
> **Design (source of truth for every screen):** [BoardAI design canvas](https://claude.ai/artifact/XY3ZrPFmFDA9gNw9Y6WuVJ) · **Architecture:** [HLD](architecture/HLD.md) · [LLD](architecture/LLD.md)

## TL;DR

BoardAI is **the board for understanding**: a voice-first desktop app where you understand, plan and design software together with an AI. Drop in a codebase, a PDF or a Markdown document and ask out loud. BoardAI explains the way a senior engineer does at a whiteboard. It speaks while it **draws hand-drawn diagrams in real time**, from a quick sketch to a detailed system map you can drill into, and shows **code that highlights in sync with what it's saying**. You can **interrupt at any moment** ("wait, wait — what's that?"), pick up the pen and draw, or say "note that". It answers, then picks up exactly where it stopped.

Software work is moving from writing code to understanding systems and working alongside AI: knowing how a system works, and identifying and balancing its trade-offs. BoardAI is built for that part. It works beside the coding harness you already use (Claude Code, pi):

- **The harness researches and writes code.** It reads the repo and returns a brief with `file:line` citations, and it makes the changes you agree on.
- **BoardAI is where you understand and decide.** It turns the research into a drawing it explains, helps you weigh a change, and captures the decisions, tasks and plans that go back to the harness to execute.

Everything you explore feeds a **personal brain** of notes, tasks, memories, session digests and replays, so each session starts where the last one ended. Over time it learns what you know and how you like to learn.

The MVP is built (§10), and it depends on three things:

1. **The voice feels human.** Latency is low, the pace is natural, and interruptions are expected, not treated as errors.
2. **Drawing is synced to speech.** The diagram grows *while* it's being explained, stroke by stroke.
3. **The UI is almost nothing.** An orb, a canvas, and surfaces that appear only when they help.

What ships after the MVP, and in what order, is in §11.

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

**One-liner:** *The board for understanding.* A whiteboard that talks back, draws while it explains, and remembers.

**Vision.** Coding agents write more of the code every month. What stays with the engineer is understanding: how a system works, which trade-offs it makes, which change to make next, and why. BoardAI is designed for understanding.

It is the board for understanding, planning, designing, mocking up and architecting software. You drop in a file, a PDF or a Markdown document and ask questions, and the explanation is drawn while it's spoken. You co-design software with the AI before anything is coded, or you understand an existing codebase by discussing it visually, in real time. You interrupt, draw, ask it to remember, and write notes with your voice. Afterwards you replay the session, discuss architectural changes, find issues, and write high-level and low-level diagrams.

Over time it becomes a personal brain. It holds your tasks, the knowledge graph of what you've explored, your memories and what it understood about your code. It also learns your expertise, your learning path, how you think a problem through, and how you like to learn. It starts with software and can extend to other complex domains later.

### 2.1 BoardAI and your coding harness

BoardAI does not replace your coding harness. Each does the part it's built for.

| | Your coding harness (Claude Code, pi) | BoardAI |
|---|---|---|
| **Its job** | Research the code, and write it | Show how it works, and help you decide |
| **What it produces** | Research briefs with `file:line` citations; changes on a git branch | Live drawn explanations, diagrams, decisions, notes, tasks and plans |
| **How you use it** | In the terminal, or driven by BoardAI | By voice, on the board |

The loop:

1. **Give BoardAI the code.** Share a folder, a crate or a file. Later, give it a repo URL and it downloads the repo.
2. **The harness researches it.** BoardAI asks Claude Code or pi for a brief, checks every line the brief cites, and draws the map.
3. **You understand and decide on the board.** You ask, interrupt, draw, compare options, and capture notes and tasks.
4. **The harness executes.** A change you confirm out loud runs in the harness on a new git branch. Later, whole tasks and designs go to it from the board.
5. **The harness pulls context back** (later). A BoardAI MCP server lets the harness search and pull the design, decisions, tasks and notes behind a request it got from BoardAI.

### 2.2 Use cases

| Use case | What it looks like | Status |
|---|---|---|
| **Understand a codebase** | "Walk me through this repo." The harness researches the shared code. BoardAI draws the module map and walks the main paths, with code cards, trace and checked `file:line` chips. | **In the MVP** |
| **Read a paper or a document** | Drop a PDF or a Markdown file and ask. BoardAI digests its text, tables and figures, redraws a figure on the board, and cites pages (`p. 5 · Figure 3`). | **In the MVP** |
| **Explain a system with detailed diagrams** | "Walk me through the whole engine." A map of 40 to 100 or more parts (nested containers, a legend, numbered steps) is revealed a part at a time as it's explained. "More detail on the arena" drills in. | **In the MVP** |
| **Draw together** | Pick up the pen and sketch your idea, name the parts by voice, say "done", and BoardAI answers about your drawing. | **In the MVP** |
| **Notes, tasks and memory by voice** | "Note that." "Add a task to check our TTLs." "Remember that we use Kafka." A digest is written when the session ends. | **In the MVP** |
| **Replay** | Open a past session in the library and scrub it like a film, or ask "show me the diagram from the Redis session". | **In the MVP** |
| **A repo from its URL** | "Use github.com/…". BoardAI downloads the repo into a folder you own and researches it through your harness, the same way as a shared folder. | **Later** (Phase 4) |
| **Co-design, from idea to code** | Brainstorm an idea → wireframes and mock-ups → a project board → software design and architecture diagrams → cloud architecture → hand it to Claude Code to implement. | **Partly built.** Architecture and system diagrams, Compare for trade-offs, and a change confirmed out loud and run by the harness work today. Brainstorming, wireframes and mock-ups, designed cloud architecture and handing a design to Claude Code are Phase 2. The project board is Phase 3. |
| **An MCP server for the coding harness** | Claude Code, working on a request from BoardAI, searches BoardAI and pulls the design, decisions and tasks behind it. | **Later** (Phase 4) |
| **The personal brain** | BoardAI knows your expertise, your learning path, how you think and how you like to learn, and adapts to them. | **Partly built.** Memory, notes, tasks, digests and recall work, and each digest records the concepts and relations of the knowledge graph. The graph view and the deeper profile are Phase 5. |
| **Project board and roadmap** | "What's next on the harness project?" A Jira-style board of columns and a roadmap timeline, called up by voice. | **Later** (Phase 3). Tasks, projects and the Now list are built. |

### 2.3 What BoardAI is and isn't

| BoardAI is | BoardAI is not |
|---|---|
| The board for understanding, planning and designing software with an AI | A coding agent that writes code for you (it hands that to one) |
| Live, interruptible, hand-drawn explanations | A diagram generator that hands you a finished image |
| A memory of everything you've explored, and of how you learn | A chat log |
| Where decisions, tasks and plans come from, captured as you talk | A project tracker you fill in by hand |
| A calm, mostly empty screen | A dashboard, an IDE, or a chat window |

## 3. Who it's for

| Persona | Job to be done | Example topics |
|---|---|---|
| **Self-directed systems learner** (primary user; we use it ourselves first) | "Make me *really* understand X, at my pace, and remember what I learned." | Linux VFS, Raft and its paper, B-trees, CPU caches, AWS networking, memory allocators |
| **Engineer onboarding onto a codebase** | "Walk me through this repo. How does a request actually flow?" | A new job, an open-source contribution |
| **Architect or engineer designing with AI** | "Talk this design through with me, show me the tradeoffs, and turn it into work for my coding agent." | Kafka vs SQS, cache strategy, a sharding plan, a feature before any code is written |
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
9. **Beside your coding harness, not instead of it.** The harness researches and writes code. BoardAI explains, helps you decide and plans, and hands the work back to the harness.

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

The MVP has one default voice, warm and unhurried, spoken by Kokoro on your Mac. The settings page switches the engine to ElevenLabs or the macOS system voice. Choosing among each engine's voices comes later.

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
- **Knowledge graph.** The graph links concepts, systems, files, projects, sessions and notes. The relationships are `explains`, `part_of`, `depends_on`, `mentioned_in` and `struggled_with`. Each session's digest records them today; the graph view comes in Phase 5 (§11).
- **Recall.** Asking "What did we cover on consensus?" or "Show me the diagram from the Redis session" brings back the canvas with a two-sentence recap.
- **Learner model.** BoardAI tracks what you've covered, what you asked about repeatedly, and your preferred pace and depth. Interruptions are a useful signal of confusion. BoardAI uses this to skip what you already know and to revisit topics where you were shaky. The MVP records these signals; the profile built from them (expertise, learning path, how you think, your preferences) comes in Phase 5.
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
- **MVP scope:** tasks, projects and the Now list. The Board and Roadmap views, and sending a task to the harness, come in Phase 3 (§11).

### 7.6 Sessions and replay

- **Sessions replay like a film.** Every session is an event log of voice, strokes and surfaces. You can scrub the timeline, which has chapters at each thread step and shows detours as branches.
- **Library.** Sessions appear as large rounded cards. Each has soft abstract cover art (a blurred pastel render of the session's diagram) and the play, mute and progress controls from the references.
- **Export.** Each session's board is saved as `.excalidraw` and its notes as Markdown, with the transcript. PNG and SVG export come later.

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

The MVP is built. Its scope is fixed: it is the right size to validate the idea and the product, and we use it ourselves first. What is left is validation (Phase 1 in §11). New functionality ships in the phases after it.

**Built**

- **A desktop app** for macOS 14+ on Apple silicon, with a light theme. Open source and bring your own keys. The repository is public, and CI runs on macOS 14.
- **Setup and settings.** A first-run screen, and a settings page (⌘, or "open settings") for the talker and its model, the scribe, the voice, the microphone and its permission, keys (checked, replaced or removed, kept in the macOS Keychain), coding harnesses and code folders, and the vault and its index.
- **Providers, all swappable:**
  - LLM: Claude, OpenRouter, or a local model through Ollama. OpenRouter takes any listed model, with measured presets (gpt-oss-120b by default, Qwen3.8 27B as the scribe, DeepSeek V4.1 Flash, Claude Haiku 5.5, Gemini 3.8 Flash and GPT-6 Luna), and the settings page shows how well each one drew when measured.
  - Speech-to-text: local Whisper with Silero voice detection, its vocabulary taken from the board and the opened code
  - Text-to-speech: Kokoro on your Mac by default, ElevenLabs (realtime), or the macOS system voice
  - Coding harness: Claude Code (headless CLI) or pi (RPC)
- **Voice loop:**
  - Open mic by default, with push-to-talk as an option
  - Barge-in using duck-then-decide, with backchannel handling
  - Captions
  - Controls by voice, such as "go back", "clear the board", "end the session" and "open settings"
- **The orb**, with every state and the orb-to-nib transition.
- **Live canvas:**
  - Architecture, flow, sequence, state, tree and data-structure diagrams (arrays, pages, hash tables)
  - Detailed diagrams, planned whole and revealed a part at a time as they're explained: nested containers, categories and a legend, records with fields, notes and numbered steps
  - Drill-down into any part ("more detail on X"), several levels deep, and "back out"
  - Human-like strokes synced to speech, strike-and-rewrite corrections, annotations and highlights
  - Pages ("clear the board"), camera follow and depth of field
- **You draw:** your shapes on the board in your own ink, labels by voice, undo, and BoardAI waits until you say "done", then answers about your drawing.
- **Explanations:** the thread, nested detours and exact resume.
- **Pointing:** hover or click while saying "this".
- **Sources (§7.7):** codebases, crates, modules, files, and Markdown, text and PDF documents, shared by drop, the chooser or voice, for a session or kept in a project, in the Sources tray.
- **Code grounding:**
  - Code understanding through research briefs from the coding harness, confined to the source's repository and cached per source. A brief's diagram is staged on the board with the harness's name and how many files it read.
  - Every cited `file:line` is read natively and checked before its chip shows
  - Repo tours and module maps that hold to what was read
  - Code cards with synced highlights and source chips
  - Trace mode
  - Reviews through the harness, read-only, and changes only after a spoken yes, on a new git branch
- **Documents:** PDF and Markdown digests with sections, tables, figures as diagrams BoardAI can redraw, and claims with pages. PDF text is read on this Mac in reading order, and figures by a model that reads files or by the harness. Page citations are checked, the Doc card shows the passage, and a figure can be staged on the board. The figure's image on the card is not built yet.
- **Surfaces:** code card, Compare, Doc card, recap, note and task chips, thread and captions.
- **Memory:** notes and tasks by voice, an automatic session digest (summary, diagrams, open questions, proposed memories, and the concepts and relations of the knowledge graph), the memory policy in §7.4, recall of past sessions, and "forget that", "don't remember this session" and "yes, keep it".
- **Views:** the library with cover art, tasks (the Now list), memory, and replay of past sessions. Projects group tasks, notes, sessions and sources.

**Out of scope for the MVP**

| Not in the MVP | Where it goes (§11) |
|---|---|
| Brainstorming, wireframes and mock-ups, designed cloud architecture, handing a design to Claude Code | Phase 2 |
| Text input | Phase 2 |
| The Board and Roadmap views, sending a task to the harness | Phase 3 |
| Repo URLs, and the BoardAI MCP server | Phase 4 |
| Visualizing the knowledge graph, the expertise and learning-path profile | Phase 5 |
| A hosted version, sync and multiplayer | Later |
| Simulations other than trace, timelines, layered stacks and plots, PNG and SVG export, a dark theme | Later |
| Windows, Linux and mobile, and domains outside software | Later |
| Editing code inside BoardAI | Never: code changes go through the coding harness |

## 11. Roadmap

The first roadmap had a Phase 0 (the magic trick: the orb, the voice loop and drawing synced to speech) and a Phase 1 (grounding and memory). The MVP built both in one push (§10). This section is the phased delivery after the MVP.

Three rules hold for every phase:

- **A phase starts when the one before it passes its exit test.** The plan tracks each phase's deliverables and exit test.
- **Each phase builds on what's built.** It reuses the MVP's voice loop, canvas, staged diagrams, sources, harness and store before adding anything new. A new seam, dependency, protocol or storage format gets its decision row first.
- **The principles in §4 don't bend.** Voice first, a calm screen, grounded claims, local-first, and code written only by the coding harness.

| Phase | What it delivers | Exit test |
|---|---|---|
| **1 · Validate the MVP** | The owner's study sessions, Claude measured, polish from use | The exit sessions pass, Claude is measured, and the polish list is closed |
| **2 · Co-design** | Brainstorming, wireframes and mock-ups, design and cloud architecture diagrams, designs handed to Claude Code, a text box | A real feature goes from an idea to code through a design made in BoardAI |
| **3 · Project planning** | The Board and Roadmap views, designs split into tasks, tasks sent to the harness | A real project is planned in BoardAI for two weeks, with its tasks done through the harness |
| **4 · MCP server and repo URLs** | A BoardAI MCP server the harness pulls from, and repos from a URL | The harness pulls a task's context itself, and a repo is learned from its URL alone |
| **5 · The personal brain** | The graph view, expertise, learning path, thinking process and preferences | After a month, the profile reads true and new sessions build on it |
| **Later** | Hosted, sync, multiplayer, beyond software | People pay for the hosted version instead of managing their own keys |

### Phase 1 · Validate the MVP

- **Delivers:**
  - **The owner's study sessions:** ten real study sessions where interrupting feels natural, and an unfamiliar open-source repo learned using only BoardAI.
  - **Claude measured** as the talker with the owner's key, for latency and drawing, the same way the OpenRouter models were. Claude's own API is measured reading PDF figures too.
  - **The checks only a person can make:** echo cancellation on laptop speakers in a quiet room, and how the voices sound.
  - **Polish from use.** Whatever the sessions show is broken, slow or confusing gets fixed, each fix with a regression test. Nothing is added to scope.
  - **A signed and notarized build**, so the first outside engineers can try it.
- **Why first:** everything after it stands on the voice loop, the drawing and the grounding. If they don't hold up in daily use, more features would only hide it.
- **Builds on:** the MVP as it is.
- **Exit test:** the ten sessions and the repo pass, Claude's numbers are on the settings page, and every item on the polish list from those sessions is fixed or deliberately deferred.

### Phase 2 · Co-design

- **Delivers:**
  - **Brainstorming.** "Let's brainstorm X" puts ideas on the board as you talk, groups them, and turns a choice between them into a Compare.
  - **Wireframes and mock-ups.** A new diagram family for screens (frames, navigation, lists, forms, buttons) in the same hand-drawn style, drawn from your words or from your own sketch.
  - **Design and architecture diagrams from the design.** Components, the data model and the main flows, high level and low level, with drill-down.
  - **Cloud architecture.** The design's deployment (regions, networks, managed services), with its trade-offs in a Compare.
  - **Design decisions by voice.** "We chose X because Y" is kept in the project, linked to the part of the board it's about.
  - **Handing a design to Claude Code.** BoardAI writes a design brief (the diagrams as structured parts, the decisions and the notes). After a spoken yes, the harness implements it on a new git branch while BoardAI narrates its progress.
  - **A text box** for names, identifiers and specs that are hard to say (§8.9).
- **Why now:** the MVP explains what already exists. Co-design uses the same board for what doesn't exist yet. It's the shortest step from the MVP, and planning (Phase 3) needs a design to plan from.
- **Builds on:** staged diagrams and drill-down, which already lay out large nested diagrams; You draw; Compare; notes and projects; and the change gate (a spoken yes, a new branch) that already sends changes to the harness.
- **Exit test:** the owner designs one real feature in BoardAI, from the idea to a wireframe and an architecture diagram, and hands it to Claude Code. The result follows the design without the owner re-explaining it in the terminal.

### Phase 3 · Project planning

- **Delivers:**
  - **Tasks with a status** (to do, doing, done), linked to the session, diagram part, design or `file:line` they came from.
  - **"Split this into tasks."** A design or a diagram becomes proposed tasks in the project, kept after you say yes.
  - **The Board view** (columns) and **the Roadmap view** (a timeline of a project's milestones), called up by voice as surfaces, never as a permanent screen (§7.5).
  - **Sending a task to the harness.** A task brief (the goal, the context, the linked diagrams and decisions, and what done means) runs in the harness on a new branch after a spoken yes. BoardAI narrates its progress, and the task moves to done with its branch linked.
  - **Planning by voice.** "What's next on the harness project?" "Move the TTL task to this week."
- **Why now:** tasks and projects are already built, and Phase 2 produces designs that need breaking down. Planning turns decisions into work the harness can do.
- **Builds on:** tasks, projects and the Now list; views called up by voice; and the design hand-off from Phase 2.
- **Exit test:** the owner plans one real project in BoardAI for two weeks with no other tracker, and at least five tasks go from the board to the harness and come back done.

### Phase 4 · The BoardAI MCP server and repo URLs

- **Delivers:**
  - **Repo URLs.** "Use github.com/…", or a dropped link, downloads the repo into a folder you own. It's shared as a code source and researched through the harness like any local folder, with the same reach and credential rules. It can be kept in a project and updated on request.
  - **The BoardAI MCP server.** A local MCP server the coding harness connects to. Its tools search the brain (notes, tasks, decisions, digests, briefs and sessions) and fetch a task or a design whole, with its diagrams as structured parts. Its only write is a task's progress and result. It runs on this Mac, is scoped to a project, and returns only what the store has already redacted.
  - **Claude Code and pi get the server** in their configuration when BoardAI starts them.
- **Why now:** after Phases 2 and 3, BoardAI holds designs, decisions and tasks worth pulling. Until then, the brief BoardAI writes is enough. Repo URLs come in the same phase because both widen what passes between BoardAI, the harness and the outside world, and both need the same care about reach and privacy.
- **Builds on:** the store's index and full-text search, cached briefs and digests, the sources model, and the harness's confinement.
- **Exit test:** Claude Code, working on a task sent from BoardAI, pulls the task's design and decisions through the MCP server instead of having them pasted into its prompt. An unfamiliar open-source repo is learned starting from only its URL.

### Phase 5 · The personal brain

- **Delivers:**
  - **The graph view** ("constellation") of concepts, systems, files, projects and sessions, with their relations. Opening a node recalls its sessions.
  - **Expertise:** what you know and how deeply, from sessions, learning signals and what you've said about yourself.
  - **A learning path:** what to learn next, from what you covered and where you struggled. The idle suggestion picks from it, and BoardAI revisits shaky topics and skips what you know.
  - **Your thinking process and preferences:** how you approach a problem (what you ask first, how you break it down), and the pace, depth and kind of examples you prefer, applied by default.
  - All of it follows the policy in §7.4: inferred items are proposed in the digest, kept only after a yes, and visible and editable in Memory.
- **Why last:** the brain learns from months of sessions, designs and plans, which Phases 1–4 produce. Built earlier, it would show an empty graph and guess from too little. It also keeps the risk of a brain that never ends (§13) contained.
- **Builds on:** the memory policy, learning signals, the entities and relations each digest already writes to the index, recall, and the Memory view.
- **Exit test:** after a month of use, the owner keeps most of the profile BoardAI proposes, and new sessions visibly build on it. BoardAI skips what's known, revisits shaky topics, and the next step on the learning path is one the owner takes.

### Later

- **Hosted:** a web version, with the same core and runtime behind a WebSocket and managed models by subscription. It starts only after the loop is validated. Its exit test: people pay for it instead of managing their own keys.
- **Sync** across devices, and **multiplayer**: shared sessions, one board that two people talk over.
- **Beyond software:** other domains (math, physics, business), a mock system design interview mode (you draw and explain while BoardAI interviews), and a mobile companion for listening and review.
- Simulations beyond trace, timelines, layered stacks and plots, PNG and SVG export, a dark theme, a choice of voices, and Windows and Linux.

## 12. Success metrics

Each phase has its own exit test (§11). These metrics run across all of them.

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
- **From understanding to work** (from Phase 2):
  - The share of designs and tasks handed to the harness that it completes without being re-explained in the terminal
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
| Scope creep: a "personal brain" never ends | Nothing gets validated | The MVP's scope is fixed and built (§10). Each phase has an exit test and starts only after the one before passes. The brain comes last (Phase 5) and grows from what sessions already record. |
| Co-design and planning turn BoardAI into an IDE or a project tracker | It loses the calm, understanding-first product | Views stay surfaces called up by voice (§7.5), code is written only by the harness, and every phase keeps §4's principles |
| The MCP server exposes the brain to a process that can write code | Private notes leak, or the harness acts on a stale plan | Local only, scoped to a project, read-mostly (its one write is a task's progress), secrets redacted before anything is stored, and a decision row before it's built |
| Cost per hour of voice | Pricing | A fast tier, prompt caching, local VAD and later local STT |
| Big labs add voice and canvas modes | Being commoditized | Depth for engineers, grounding in *your* code, drawing built around interruptions, local-first memory |

## 14. Landscape (adjacent products, not equivalents)

| Category | Examples | What BoardAI adds |
|---|---|---|
| General voice assistants | ChatGPT Voice, Gemini Live | Diagrams drawn in sync with speech, and knowledge of your repo |
| AI explainers | NotebookLM audio and video overviews | A live conversation you can interrupt, instead of pre-rendered media |
| AI diagramming | Excalidraw and tldraw AI features, Eraser, Napkin | A narrating teacher; drawing that happens live |
| Design whiteboards | FigJam, Miro | A co-designer that talks and draws, knows your code, and hands the design to your coding agent (Phase 2) |
| Coding agents | Claude Code, Cursor, pi | Built for understanding and deciding rather than changing code; it hands the changes to them, and uses them underneath |
| Project trackers | Jira, Linear | Tasks captured from the conversation, linked to the diagram and the code, and sent to the harness (Phase 3) |
| Notes and PKM | Obsidian, Notion, Tana | Memory captured automatically from conversation |

**The wedge** is the combination: live, interruptible voice plus drawing, grounded in your code, with memory, beside the coding harness you already use. It's for engineers first.

## 15. Decisions

The open questions from the first draft are answered. The decision log (kept outside this repository, cited as `D-123`) records each decision with its reason.

| Question | Decision |
|---|---|
| Name | **BoardAI.** "openboard" was also proposed, but OpenBoard is already an established open-source whiteboard app. |
| Voice pipeline | Cascaded (STT → LLM → TTS) for the MVP |
| Desktop shell | Tauri |
| Canvas | A custom renderer with an Excalidraw-compatible format |
| Mic | Both modes. Open mic is the default; push-to-talk is an option. |
| Voice | One default voice (Kokoro); the settings page switches the engine to ElevenLabs or the system voice. A choice among each engine's voices comes later. |
| Positioning | The board for understanding, beside the coding harness: the harness researches and writes code, and BoardAI explains, helps you decide, and plans (§2) |
| Roadmap | The MVP's scope is fixed at what's built (§10). New functionality ships in the phases of §11, each gated by its exit test. |
| Business model | Open source and bring your own keys, with a choice of providers (§10). A hosted version with managed models and sync comes after the MVP is validated (Later, §11). |
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
| **Coding harness** | The coding agent you already use (Claude Code, pi). It researches and writes code for BoardAI. |
| **Source** | Code or a document you share, for one session or kept in a project |
| **Brief** | The harness's research on a code source: components, flows with `file:line`, and a diagram |
| **Digest** | A document's summary: sections, tables, figures as diagrams, and claims with pages. A session also gets a digest when it ends. |
| **MCP server** | (Phase 4) The local server the coding harness uses to search and pull from BoardAI |
