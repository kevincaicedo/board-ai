// The numeric design tokens TypeScript needs (sizes and timings for the imperative renderers and
// layout math). tokens.css is the only place their values live: this module reads them from the
// computed style at boot and fails loudly when one is missing, instead of carrying a fallback.

/** Each token's CSS name and unit; `px` and `ms` values are stored as plain numbers. */
const TOKEN_SPEC = {
  durationMicroMs: ["--duration-micro", "ms"],
  durationStandardMs: ["--duration-standard", "ms"],
  durationSceneMs: ["--duration-scene", "ms"],
  durationBandInMs: ["--duration-band-in", "ms"],
  durationThreadInMs: ["--duration-thread-in", "ms"],
  durationBranchInMs: ["--duration-branch-in", "ms"],
  durationOrbCondenseMs: ["--duration-orb-condense", "ms"],
  durationNibFlightMs: ["--duration-nib-flight", "ms"],
  durationNibHomeMs: ["--duration-nib-home", "ms"],
  durationDockGrowMs: ["--duration-dock-grow", "ms"],
  durationDockSwellMs: ["--duration-dock-swell", "ms"],
  durationDetourCardDelayMs: ["--duration-detour-card-delay", "ms"],
  durationSurfaceDelayMs: ["--duration-surface-delay", "ms"],
  durationChipFlyMs: ["--duration-chip-fly", "ms"],
  durationChipConfirmMs: ["--duration-chip-confirm", "ms"],
  durationChipStaggerMs: ["--duration-chip-stagger", "ms"],
  durationChipHoldMaxMs: ["--duration-chip-hold-max", "ms"],
  durationCaptionLingerMs: ["--duration-caption-linger", "ms"],
  durationCaptionFadeMs: ["--duration-caption-fade", "ms"],
  durationCaptionWordMs: ["--duration-caption-word", "ms"],
  durationCaptionPreviousLingerMs: ["--duration-caption-previous-linger", "ms"],
  durationCaptionPreviousFadeMs: ["--duration-caption-previous-fade", "ms"],
  durationCellMarkMs: ["--duration-cell-mark", "ms"],
  durationCellTextMs: ["--duration-cell-text", "ms"],
  durationMicOnMs: ["--duration-mic-on", "ms"],
  durationMicOffMs: ["--duration-mic-off", "ms"],
  durationGlowDelayMs: ["--duration-glow-delay", "ms"],
  durationSuggestionInMs: ["--duration-suggestion-in", "ms"],
  durationSuggestionOutMs: ["--duration-suggestion-out", "ms"],
  durationCodeMarkMs: ["--duration-code-mark", "ms"],
  durationViewStaggerMs: ["--duration-view-stagger", "ms"],
  typeCaption: ["--type-caption", "px"],
  typeCaptionPrevious: ["--type-caption-previous", "px"],
  handNode: ["--hand-node", "px"],
  handNodeSmall: ["--hand-node-small", "px"],
  handNodeSmallBelow: ["--hand-node-small-below", "px"],
  handGroup: ["--hand-group", "px"],
  handEdge: ["--hand-edge", "px"],
  handAnnotation: ["--hand-annotation", "px"],
  handIndex: ["--hand-index", "px"],
  handDash: ["--hand-dash", "px"],
  handDashGap: ["--hand-dash-gap", "px"],
  orbSize: ["--orb-size", "px"],
  orbCenterLift: ["--orb-center-lift", "px"],
  orbDockBottom: ["--orb-dock-bottom", "px"],
  orbDockSize: ["--orb-dock-size", "px"],
  orbDockSwell: ["--orb-dock-swell", "px"],
  nibSize: ["--nib-size", "px"],
  threadTop: ["--thread-top", "px"],
  threadStepGap: ["--thread-step-gap", "px"],
  surfaceWidth: ["--surface-width", "px"],
  surfaceRight: ["--surface-right", "px"],
  surfaceTop: ["--surface-top", "px"],
  surfaceCenter: ["--surface-center", "px"],
  surfaceGap: ["--surface-gap", "px"],
  surfaceRise: ["--surface-rise", "px"],
  surfaceBlurIn: ["--surface-blur-in", "px"],
  surfaceBlurOut: ["--surface-blur-out", "px"],
  detourCardHeight: ["--detour-card-height", "px"],
  codeHead: ["--code-head", "px"],
  codeLine: ["--code-line", "px"],
  codeFoot: ["--code-foot", "px"],
  compareHead: ["--compare-head", "px"],
  compareRow: ["--compare-row", "px"],
  compareFoot: ["--compare-foot", "px"],
  recapHead: ["--recap-head", "px"],
  recapRow: ["--recap-row", "px"],
  recapFoot: ["--recap-foot", "px"],
  detourPushBlur: ["--detour-push-blur", "px"],
  detourPushShift: ["--detour-push-shift", "px"],
  detourPushShrink: ["--detour-push-shrink", "number"],
  detourPushFade: ["--detour-push-fade", "number"],
  dofBlur: ["--dof-blur", "px"],
  dofFade: ["--dof-fade", "number"],
  captionBottom: ["--caption-bottom", "px"],
  suggestionRise: ["--suggestion-rise", "px"],
  suggestionBlur: ["--suggestion-blur", "px"],
  boardTop: ["--board-top", "px"],
  boardBottom: ["--board-bottom", "px"],
  boardSide: ["--board-side", "px"],
  detourBoardTop: ["--detour-board-top", "px"],
  detourBoardBottom: ["--detour-board-bottom", "px"],
  detourBoardSide: ["--detour-board-side", "px"],
  cameraScaleMax: ["--camera-scale-max", "number"],
  captionRise: ["--caption-rise", "px"],
  captionPreviousRise: ["--caption-previous-rise", "px"],
  captionMaxLetters: ["--caption-max-letters", "number"],
  chipLeft: ["--chip-left", "px"],
  chipBottom: ["--chip-bottom", "px"],
  chipGap: ["--chip-gap", "px"],
  chipTargetRight: ["--chip-target-right", "px"],
  chipTargetLift: ["--chip-target-lift", "px"],
  chipTargetStepX: ["--chip-target-step-x", "px"],
  chipTargetStepY: ["--chip-target-step-y", "px"],
  micRing: ["--mic-ring", "px"],
  micRingVoice: ["--mic-ring-voice", "px"],
  viewFreeMin: ["--view-free-min", "px"],
  panelRight: ["--panel-right", "px"],
  panelWidth: ["--panel-width", "px"],
  panelMemoryRight: ["--panel-memory-right", "px"],
  panelMemoryWidth: ["--panel-memory-width", "px"],
  panelMemoryGap: ["--panel-memory-gap", "px"],
  panelShiftIn: ["--panel-shift-in", "px"],
  panelBlurIn: ["--panel-blur-in", "px"],
  viewOrbTasks: ["--view-orb-tasks", "px"],
  viewOrbTasksX: ["--view-orb-tasks-x", "number"],
  viewOrbMemory: ["--view-orb-memory", "px"],
  viewOrbMemoryX: ["--view-orb-memory-x", "number"],
  recallCardWidth: ["--recall-card-width", "px"],
  recallBoardSoften: ["--recall-board-soften", "px"],
  captionWidth: ["--caption-width", "px"],
  setupOrb: ["--setup-orb", "px"],
} as const satisfies Record<string, readonly [string, "px" | "ms" | "number"]>;

export type Tokens = { readonly [K in keyof typeof TOKEN_SPEC]: number };

/** What `getComputedStyle(document.documentElement)` offers, and what tests fake. */
export interface TokenSource {
  getPropertyValue(name: string): string;
}

export class MissingTokensError extends Error {
  readonly names: readonly string[];

  constructor(names: readonly string[]) {
    super(`design tokens missing or malformed in tokens.css: ${names.join(", ")}`);
    this.name = "MissingTokensError";
    this.names = names;
  }
}

const NUMBER = /^-?(\d+\.?\d*|\.\d+)$/;

/**
 * Parses a token's computed value. The production build minifies tokens.css (900ms becomes .9s,
 * 0.035 becomes .035), so times accept seconds and numbers may lose their leading zero.
 */
function parseToken(raw: string, unit: "px" | "ms" | "number"): number | null {
  const text = raw.trim();
  let body = text;
  let scale = 1;
  if (unit === "px") {
    if (!text.endsWith("px")) {
      return text === "0" ? 0 : null;
    }
    body = text.slice(0, -2);
  } else if (unit === "ms") {
    if (text.endsWith("ms")) {
      body = text.slice(0, -2);
    } else if (text.endsWith("s")) {
      body = text.slice(0, -1);
      scale = 1000;
    } else {
      return null;
    }
  }
  if (!NUMBER.test(body)) {
    return null;
  }
  return Math.round(Number(body) * scale * 1000) / 1000;
}

export function readTokens(source: TokenSource): Tokens {
  const values: Record<string, number> = {};
  const missing: string[] = [];
  for (const [key, [name, unit]] of Object.entries(TOKEN_SPEC)) {
    const value = parseToken(source.getPropertyValue(name), unit);
    if (value === null) {
      missing.push(name);
    } else {
      values[key] = value;
    }
  }
  if (missing.length > 0) {
    throw new MissingTokensError(missing);
  }
  return values as Tokens;
}

/** Custom properties declared in a `:root { … }` block of a stylesheet's text. */
export function tokenSourceFromCss(css: string): TokenSource {
  const declarations = new Map<string, string>();
  const root = /:root\s*\{([^}]*)\}/.exec(css);
  const body = root?.[1] ?? "";
  for (const match of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    const name = match[1];
    const value = match[2];
    if (name !== undefined && value !== undefined) {
      declarations.set(name, value.trim());
    }
  }
  return { getPropertyValue: (name) => declarations.get(name) ?? "" };
}
