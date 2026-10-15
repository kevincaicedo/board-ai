// Routes each protocol event to the part of the UI that shows it: the board model, the orb's
// presence and dynamics, and the Preact session store. It is the transport's sink, so a replay
// reset or a live batch lands in one place.

import { BoardModel, type LeadDecision } from "../canvas/board.ts";
import { leadDecision, Presence } from "../canvas/presence.ts";
import { OrbDynamics } from "../orb/dynamics.ts";
import type { UiBatch } from "../protocol/generated/UiBatch.ts";
import type { UiEvent } from "../protocol/generated/UiEvent.ts";
import type { Tokens } from "../styles/tokens.ts";
import type { BatchSink } from "../transport/transport.ts";
import { SessionStore } from "./session.ts";

/** Session time a popped detour layer is kept after its exit, then dropped. */
const POPPED_LAYER_KEEP_MS = 5_000;

export class Scene implements BatchSink {
  readonly board: BoardModel;
  readonly presence: Presence;
  readonly orb = new OrbDynamics();
  readonly store: SessionStore;
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
      case "Camera":
        this.board.apply(event, at, this.#lead);
        break;
      case "LayerPush":
      case "LayerPop":
        this.board.apply(event, at, this.#lead);
        break;
      case "Caption":
      case "Thread":
      case "SurfaceShow":
      case "SurfaceUpdate":
      case "SurfaceDismiss":
      case "Chip":
      case "Problem":
        break;
    }
    this.store.apply(event, at);
  }
}
