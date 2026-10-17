import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import type { SourceRow } from "../protocol/generated/SourceRow.ts";
import type { SourcesTray } from "../protocol/generated/SourcesTray.ts";
import { parseUiEvent } from "../protocol/parse.ts";
import { PROJECT_ROWS_MAX } from "../protocol/sources.ts";
import { readTokens, tokenSourceFromCss } from "../styles/tokens.ts";
import {
  badgeOf,
  dropPills,
  dropTitle,
  firstTab,
  footerLine,
  headerLeft,
  pinLabel,
  rowsOf,
  SourcesStore,
  stateLine,
  type TrayCorner,
  tabLabels,
  toneOf,
  trayCorner,
  trayPresence,
  trayWidth,
} from "./model.ts";

const tokens = readTokens(
  tokenSourceFromCss(readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8")),
);

function row(id: number, title: string, state: SourceRow["state"]): SourceRow {
  return { id, kind: { kind: "CodeFolder" }, title, path: `~/…/infinitydb/crates/${title}`, state };
}

const reading: SourceRow["state"] = {
  state: "Reading",
  reader: "Claude Code",
  files_read: 23,
  files_estimate: 40,
};

/** Artboard 14: inf-alloc being read, the paper read, and the infinitydb project's three. */
function tray(open: boolean): SourcesTray {
  return {
    open,
    session: [
      row(1, "inf-alloc", reading),
      {
        id: 2,
        kind: { kind: "Document", format: "Pdf" },
        title: "FoundationDB paper",
        path: "~/Papers/foundationdb-sigmod21.pdf",
        state: { state: "Waiting", reason: "Waiting for the document reader" },
      },
    ],
    project: {
      name: "infinitydb",
      proposed: false,
      sources: [
        row(3, "infinitydb", { state: "Ready", detail: "Overview ready · 17 parts mapped" }),
        row(4, "storage", {
          state: "Partial",
          reason: "Overview ready · 2 citations couldn't be checked",
        }),
        row(5, "gone", { state: "Failed", reason: "Not found any more" }),
      ],
    },
    talker: "OpenRouter",
    harness: "Claude Code",
    reading: ["inf-alloc/src/arena.rs", "src/pool/aligned.rs"],
  };
}

test("each row says its state as the canvas does", () => {
  assert.equal(stateLine(reading), "Claude Code is reading · 23 of ~40 files");
  assert.equal(
    stateLine({ state: "Reading", reader: "pi", files_read: 0, files_estimate: null }),
    "pi is reading",
  );
  assert.equal(
    stateLine({ state: "Reading", reader: "pi", files_read: 1, files_estimate: null }),
    "pi is reading · 1 file",
  );
  const project = tray(true).project?.sources ?? [];
  assert.deepEqual(
    project.map((each) => [stateLine(each.state), toneOf(each.state)]),
    [
      ["Overview ready · 17 parts mapped", "ready"],
      ["Overview ready · 2 citations couldn't be checked", "partial"],
      ["Not found any more", "failed"],
    ],
  );
  assert.equal(toneOf(reading), "reading");
});

test("the tabs count what each scope holds, and a proposed project says so", () => {
  const shown = tray(true);
  assert.deepEqual(tabLabels(shown), {
    session: "This session · 2",
    project: "infinitydb project · 3",
  });
  const proposed: SourcesTray = {
    ...shown,
    project: { name: "inf-alloc", proposed: true, sources: [] },
  };
  assert.equal(tabLabels(proposed).project, "inf-alloc project? · 0");
  assert.equal(tabLabels({ ...shown, project: null }).project, null);
  assert.deepEqual(
    rowsOf(shown, "project").map((each) => each.id),
    [3, 4, 5],
  );
  assert.equal(firstTab(shown), "session");
  assert.equal(firstTab({ ...shown, session: [] }), "project");
});

test("badges, pins and the footer name what the canvas names", () => {
  assert.deepEqual(badgeOf({ kind: "CodeFolder" }, false), { text: "{ }", tint: "code" });
  assert.deepEqual(badgeOf({ kind: "CodeFolder" }, true), { text: "{ }", tint: "kept" });
  assert.deepEqual(badgeOf({ kind: "Document", format: "Pdf" }, true), {
    text: "PDF",
    tint: "pdf",
  });
  assert.deepEqual(badgeOf({ kind: "Document", format: "Markdown" }, false), {
    text: "MD",
    tint: "markdown",
  });
  assert.equal(pinLabel(false, "infinitydb"), "Keep in the infinitydb project");
  assert.equal(pinLabel(true, "infinitydb"), "Kept in the infinitydb project");
  assert.equal(pinLabel(false, null), "Keep in a new project");
  assert.equal(
    footerLine(tray(true)),
    "Read on this Mac. Text goes only to OpenRouter (talking) and Claude Code (research).",
  );
  assert.equal(
    footerLine({ ...tray(true), talker: "the local model", harness: null }),
    "Read on this Mac. Text goes only to the local model (talking).",
  );
});

test("the drop overlay lights the half under the pointer, and says why it can't share", () => {
  const overlay = {
    title: "hekaton-sigmod13.pdf",
    refused: null,
    half: "Session" as const,
    project: "infinitydb",
  };
  assert.equal(dropTitle(overlay), "Drop to share hekaton-sigmod13.pdf");
  assert.deepEqual(dropPills(overlay), [
    { half: "Session", label: "This session", on: true },
    { half: "Project", label: "Keep in infinitydb", on: false },
  ]);
  assert.deepEqual(
    dropPills({ ...overlay, half: "Project" }).map((pill) => pill.on),
    [false, true],
  );
  assert.equal(
    dropTitle({ ...overlay, refused: "Can't share that: it's where apps keep their data." }),
    "Can't share that: it's where apps keep their data.",
  );
});

test("the tray never sits over the column or a view", () => {
  assert.equal(trayWidth(1440, false, false, tokens), tokens.trayWidth);
  assert.equal(trayWidth(1440, true, false, tokens), tokens.trayWidth, "room left of the column");
  assert.equal(trayWidth(1440, false, true, tokens), null, "a view is open");
  const narrow = trayWidth(1120, true, false, tokens);
  assert.ok(narrow !== null && narrow < tokens.trayWidth && narrow >= tokens.trayWidthMin);
  assert.equal(trayWidth(960, true, false, tokens), null, "no room beside the column");
  assert.equal(trayWidth(960, false, false, tokens), tokens.trayWidth);
  // The column's left edge stays clear by the gap at every width the tray shows.
  for (let width = 960; width <= 2000; width += 20) {
    const shown = trayWidth(width, true, false, tokens);
    if (shown !== null) {
      const columnLeft = width - tokens.surfaceRight - tokens.surfaceWidth;
      assert.ok(tokens.trayLeft + shown + tokens.trayColumnGap <= columnLeft, `${width}`);
    }
  }
});

test("the tray eases in and out on session time", () => {
  assert.equal(trayPresence(null, null, 500, 900, 420), 0);
  assert.equal(trayPresence(1000, null, 1000, 900, 420), 0);
  assert.equal(trayPresence(1000, null, 1900, 900, 420), 1);
  const leaving = trayPresence(1000, 3000, 3210, 900, 420);
  assert.ok(leaving > 0 && leaving < 1);
  assert.equal(trayPresence(1000, 3000, 3420, 900, 420), 0);
  assert.equal(trayPresence(4000, 3000, 4900, 900, 420), 1, "reopened after a close");
});

test("the store follows the core's tray, and the drop overlay comes and goes", () => {
  const store = new SourcesStore();
  store.apply({ type: "Sources", tray: tray(false) }, 100);
  assert.equal(store.openAt.value, null);
  store.tab.value = "project";
  store.apply({ type: "Sources", tray: { ...tray(true), session: [] } }, 1000);
  assert.equal(store.openAt.value, 1000);
  assert.equal(store.tab.value, "project", "an empty session opens on the project's tab");
  store.apply({ type: "Sources", tray: tray(true) }, 1200);
  assert.equal(store.openAt.value, 1000, "still open: no new entrance");
  store.apply({ type: "Sources", tray: tray(false) }, 2000);
  assert.equal(store.closedAt.value, 2000);
  store.apply(
    { type: "SourceDrop", drop: { title: "x.pdf", refused: null, half: "Project", project: "p" } },
    2100,
  );
  assert.equal(store.drop.value?.half, "Project");
  store.apply({ type: "SourceDrop", drop: null }, 2200);
  assert.equal(store.drop.value, null);
  store.apply({ type: "Sources", tray: tray(true) }, 3000);
  store.apply({ type: "SessionEnd" }, 3500);
  assert.equal(store.closedAt.value, 3500, "the session's end closes the tray");
  store.reset();
  assert.equal(store.tray.value, null);
});

test("the tray and the overlay parse at the boundary, a bad row skipped", () => {
  const skips = { rows: 0 };
  const raw = JSON.parse(JSON.stringify({ type: "Sources", tray: tray(true) }));
  raw.tray.session.push({ id: "not a number" });
  const event = parseUiEvent(raw, skips);
  assert.equal(event?.type, "Sources");
  assert.equal(skips.rows, 1);
  if (event?.type === "Sources") {
    assert.equal(event.tray.session.length, 2);
    assert.equal(event.tray.project?.sources.length, 3);
  }
  const tooMany = JSON.parse(JSON.stringify({ type: "Sources", tray: tray(true) }));
  tooMany.tray.project.sources = Array.from({ length: PROJECT_ROWS_MAX + 1 }, (_, id) =>
    row(id, "x", reading),
  );
  assert.equal(parseUiEvent(tooMany), null, "past the core's bound it's another build's");
  assert.deepEqual(
    parseUiEvent({
      type: "SourceDrop",
      drop: { title: "a", refused: null, half: "Session", project: "p" },
    }),
    { type: "SourceDrop", drop: { title: "a", refused: null, half: "Session", project: "p" } },
  );
  assert.deepEqual(parseUiEvent({ type: "SourceDrop", drop: null }), {
    type: "SourceDrop",
    drop: null,
  });
  assert.equal(
    parseUiEvent({
      type: "SourceDrop",
      drop: { title: "a", refused: null, half: "Up", project: "p" },
    }),
    null,
  );
});

/**
 * Whether a stage header starting at `left` overlaps what holds the top-left corner. Both sit
 * in the same band at the top, so only their horizontal extents matter; the tray's box is where
 * it is drawn this frame, shifted in from the left as it eases in (`--tray-shift-in`).
 */
function overlaps(corner: TrayCorner, left: number, shiftIn: number): boolean {
  const button = corner.buttonShown && left < tokens.trayLeft + tokens.sizeControl;
  const shown = corner.width !== null && corner.presence > 0;
  const trayRight =
    corner.width === null ? 0 : tokens.trayLeft + corner.width - (1 - corner.presence) * shiftIn;
  return button || (shown && left < trayRight);
}

test("the stage header never overlaps the tray or its button (LLD §4)", () => {
  // The tray's own entrance shift, from its block of tokens.css (not a TypeScript token).
  const css = readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8");
  const shiftIn = Number(/--tray-shift-in:\s*(\d+)px/.exec(css)?.[1]);
  assert.ok(shiftIn > 0);
  const at = (presence: number, width: number | null, buttonShown: boolean): TrayCorner => ({
    buttonShown,
    width,
    presence,
  });
  // The planted violation: the header at its canvas place, under an open tray or beside the
  // button, overlaps; the check sees it.
  assert.ok(overlaps(at(1, tokens.trayWidth, false), tokens.stageHeaderLeft, shiftIn));
  assert.ok(overlaps(at(0, tokens.trayWidth, true), tokens.stageHeaderLeft, shiftIn));
  // Nothing in the corner (a replay, the tray closed): the header keeps its place.
  assert.equal(
    headerLeft({ buttonShown: false, width: tokens.trayWidth, presence: 0 }, tokens),
    tokens.stageHeaderLeft,
  );
  for (let stageWidth = 960; stageWidth <= 2000; stageWidth += 20) {
    for (const columnOpen of [false, true]) {
      for (const viewOpen of [false, true]) {
        for (const replay of [false, true]) {
          for (const open of [false, true]) {
            let previous = Number.NEGATIVE_INFINITY;
            for (let step = 0; step <= 20; step += 1) {
              const ms = step * 50;
              const corner = trayCorner(
                {
                  open,
                  openAt: open ? 0 : -10_000,
                  closedAt: open ? null : 0,
                  at: ms,
                  stageWidth,
                  columnOpen,
                  viewOpen,
                  replay,
                },
                tokens,
              );
              const left = headerLeft(corner, tokens);
              const label = `${stageWidth} column ${columnOpen} view ${viewOpen} replay ${replay} open ${open} at ${ms}`;
              // Settled, or once the tray is more than faintly there: never over it.
              if (corner.presence >= tokens.stageHeaderTrayLead || corner.presence === 0) {
                assert.ok(!overlaps(corner, left, shiftIn), label);
              }
              assert.ok(left >= tokens.stageHeaderLeft, label);
              // It follows the tray without jumping back: rightward as it opens, leftward as it
              // closes.
              if (open) {
                assert.ok(left >= previous, label);
              } else if (previous !== Number.NEGATIVE_INFINITY) {
                assert.ok(left <= previous, label);
              }
              previous = left;
            }
          }
        }
      }
    }
  }
  // Beside the open tray by the gap; beside the button by the same gap.
  assert.equal(
    headerLeft({ buttonShown: false, width: tokens.trayWidth, presence: 1 }, tokens),
    tokens.trayLeft + tokens.trayWidth + tokens.stageHeaderTrayGap,
  );
  assert.equal(
    headerLeft({ buttonShown: true, width: tokens.trayWidth, presence: 0 }, tokens),
    tokens.trayLeft + tokens.sizeControl + tokens.stageHeaderTrayGap,
  );
});
