// Routes each protocol event to the part of the UI that shows it: the board model, the orb's
// presence and dynamics, and the Preact session store. It is the transport's sink, so a replay
// reset or a live batch lands in one place.

import { BoardModel, type LeadDecision } from "../canvas/board.ts";
import { leadDecision, Presence } from "../canvas/presence.ts";
import { DrawModel } from "../draw/model.ts";
import { OrbDynamics } from "../orb/dynamics.ts";
import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import { SourcesStore } from "../sources/model.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { BatchSink } from "../transport/transport.ts";
import { DiagramStore } from "./diagrams.ts";
import { SessionStore } from "./session.ts";
import { ViewStore } from "./views.ts";

/** Session time a popped detour layer is kept after its exit, then dropped. */
const POPPED_LAYER_KEEP_MS = 5_000;

export class Scene implements BatchSink {
  readonly board: BoardModel;
  readonly presence: Presence;
  readonly orb = new OrbDynamics();
  readonly store: SessionStore;
  /** The view summoned by voice (library, Now list, memory, a past board, a replay). */
  readonly views = new ViewStore();
  /** Drawing together (13 · You draw): the mode, your shapes and the label hint. */
  readonly draw = new DrawModel();
  /** Staged diagrams' headers and the open detail layers (15, 15b). */
  readonly diagrams = new DiagramStore();
  /** The Sources tray and the drop overlay (14 · Sources, 14b). */
  readonly sources = new SourcesStore();
  /** Bumped on reset, so views know to drop what they built. */
  generation = 0;
  readonly #lead: LeadDecision;

  constructor(tokens: Tokens) {
    this.board = new BoardModel(tokens);
    this.presence = new Presence(tokens);
    this.store = new SessionStore(tokens);
    this.#lead = leadDecision(this.board, this.presence);
  }

  batch(batch: UiBatch): void {
    for (const event of batch.events) {
      this.apply(event, batch.at);
    }
    this.board.prune(batch.at, POPPED_LAYER_KEEP_MS);
  }

  reset(): void {
    this.board.reset();
    this.presence.reset();
    this.orb.reset();
    this.store.reset();
    this.views.reset();
    this.draw.reset();
    this.diagrams.reset();
    this.sources.reset();
    this.generation += 1;
  }

  apply(event: UiEvent, at: number): void {
    switch (event.type) {
      case "Orb":
        this.presence.orb(event.state, event.level_percent, at);
        this.orb.set(event.state, event.level_percent, at);
        break;
      case "Acknowledge":
        this.presence.acknowledge(at);
        break;
      case "OpStart":
        if (event.layer === 0) {
          this.presence.drawingStarted(at);
        }
        this.board.apply(event, at, this.#lead);
        break;
      case "Freeze":
        this.presence.freeze(at);
        this.board.apply(event, at, this.#lead);
        break;
      case "Resume":
        this.board.apply(event, at, this.#lead);
        break;
      case "Camera": {
        // Shapes you drew before the layer had a frame ease into it rather than jump.
        const drawn = (this.draw.layers.get(event.layer)?.size ?? 0) > 0;
        this.board.apply(event, at, this.#lead, drawn);
        break;
      }
      case "LayerPush":
      case "LayerPop":
        this.board.apply(event, at, this.#lead);
        break;
      case "PageTurn":
        this.board.apply(event, at, this.#lead);
        this.draw.turnPage();
        break;
      case "ViewShow":
        this.views.show(event.view, event.today, at);
        break;
      case "ViewDismiss":
        this.views.dismiss(at);
        break;
      // The session ended (D-145): the board fades out, the orb returns to the center, and
      // the open view leaves; the session store lets its cards and lines go.
      case "SessionEnd":
        this.board.end(at);
        this.presence.rest(at);
        this.views.dismiss(at);
        // A drawing ends with its session; your shapes fade with the board.
        this.draw.end();
        break;
      // A new session's stage is reset by the transport before this event arrives (a replay
      // starts with it at time zero, on an empty stage already).
      case "SessionStart":
      case "Caption":
      case "Thread":
      case "SurfaceShow":
      case "SurfaceUpdate":
      case "SurfaceDismiss":
      case "Chip":
      case "Problem":
      case "OpenSettings":
      case "Diagram":
        break;
      case "DrawMode":
        // BoardAI watches from the dock while you draw, as it does after drawing itself.
        if (event.drawing) {
          this.presence.drawingStarted(at);
        }
        this.draw.apply(event, at);
        break;
      case "UserShape":
      case "UserShapeRemove":
      case "LabelHint":
        this.draw.apply(event, at);
        break;
      case "Sources":
      case "SourceDrop":
        break;
    }
    this.store.apply(event, at);
    this.diagrams.apply(event, at);
    this.sources.apply(event, at);
  }
}
