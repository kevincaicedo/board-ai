// The settings page (artboard 12 · Settings, LLD §3.8): ⌘, or "open settings" over the running
// stage. A nav of sections with "Back to talking", and the sections: the talker and its model,
// the voice, the microphone, the keys, coding, and memory and privacy. Each choice saves at
// once; leaving (Esc, ⌘, or "Back to talking") finishes setup again, and the app restarts the
// session when something changed (D-103).

import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
// Imported here rather than in main.tsx: the page is its only user.
import "../styles/settings.css";
import { opensSettings } from "../app/keys.ts";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import { Problem } from "./cards.tsx";
import { type ModelCatalog, sourceFor } from "./catalog.ts";
import type { SetupController } from "./controller.ts";
import { mountSetupOrb } from "./orb.ts";
import {
  CodingSection,
  KeysSection,
  MemorySection,
  MicrophoneSection,
  VoiceSection,
} from "./SettingsSections.tsx";
import { ThinkingSection } from "./SettingsThinking.tsx";
import {
  activeSection,
  BACK_TO_TALKING,
  SECTIONS,
  SETTINGS_HINT,
  SETTINGS_LEDE,
  SETTINGS_TITLE,
  type Section,
  sectionBadge,
} from "./settingsWords.ts";
import { savesInFlight } from "./state.ts";
import { startHint } from "./words.ts";

function sectionId(section: Section): string {
  return `settings-${section}`;
}

export function SettingsPage({
  controller,
  catalog,
  still,
}: {
  controller: SetupController;
  catalog: ModelCatalog;
  still: boolean;
}) {
  const state = controller.state.value;
  const status = state.status;
  const page = useRef<HTMLElement>(null);
  const main = useRef<HTMLDivElement>(null);
  const orbHost = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<Section>("thinking");
  const [openKey, setOpenKey] = useState<KeyName | null>(null);
  const picked = useRef<Section | null>(null);

  useEffect(() => {
    page.current?.focus();
  }, []);

  // Esc and ⌘, leave from anywhere while the page is open, even with focus back on the body
  // after a focused control went away: a listener on the page's own element missed those. The
  // stage's keys don't need stopping here; they wait on `settingsOpen` (keys.ts).
  useEffect(() => {
    const leave = (event: KeyboardEvent): void => {
      const shortcut = opensSettings({
        code: event.code,
        meta: event.metaKey,
        repeat: event.repeat,
      });
      if ((event.code === "Escape" && !event.repeat) || shortcut) {
        event.preventDefault();
        // ⌘, would otherwise reach installSettings' listener, which only opens.
        event.stopPropagation();
        void controller.leave();
      }
    };
    window.addEventListener("keydown", leave, { capture: true });
    return () => window.removeEventListener("keydown", leave, { capture: true });
  }, [controller]);

  useLayoutEffect(() => {
    const host = orbHost.current;
    // The host's size is the token (--settings-orb); the orb draws to it.
    return host === null ? undefined : mountSetupOrb(host, host.clientWidth, still);
  }, [still]);

  useEffect(() => {
    void controller.harnessVersions();
    // Coming back from System Settings, Finder or the Keychain.
    const again = (): void => void controller.refresh();
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, [controller]);

  const provider = status?.talker.provider ?? null;
  const model = status?.talker.model ?? null;
  useEffect(() => {
    if (provider !== null) {
      void catalog.show(sourceFor(provider));
    }
  }, [catalog, provider]);
  useEffect(() => {
    if (provider === "OpenRouter" && model !== null) {
      void catalog.loadProviders(model);
    }
  }, [catalog, provider, model]);

  const goTo = (section: Section): void => {
    picked.current = section;
    setActive(section);
    document.getElementById(sectionId(section))?.scrollIntoView({ block: "start" });
  };
  const addKey = (key: KeyName): void => {
    setOpenKey(key);
    goTo("keys");
  };
  const followScroll = (): void => {
    const column = main.current;
    if (column === null) {
      return;
    }
    const top = column.getBoundingClientRect().top;
    const tops = SECTIONS.map(({ id }) => ({
      section: id,
      top: (document.getElementById(sectionId(id))?.getBoundingClientRect().top ?? 0) - top,
    }));
    const atEnd = column.scrollTop + column.clientHeight >= column.scrollHeight - 1;
    setActive(activeSection(tops, atEnd, picked.current));
  };

  const waiting = state.problems.start === null || status === null ? null : startHint(status);
  return (
    <main class="settings" aria-label="Settings" ref={page} tabIndex={-1}>
      <nav class="settings-nav" aria-label="Settings sections">
        <div class="settings-brand">
          <div class="settings-orb" ref={orbHost} />
          <span class="settings-brand-title">Settings</span>
        </div>
        <div class="settings-links">
          {SECTIONS.map((section) => (
            <a
              key={section.id}
              href={`#${sectionId(section.id)}`}
              class="settings-link"
              aria-current={active === section.id ? "true" : undefined}
              onClick={(event) => {
                event.preventDefault();
                goTo(section.id);
              }}
            >
              <span class="settings-link-dot" />
              {section.label}
              <span class="settings-badge">
                {status === null ? "" : sectionBadge(section.id, status)}
              </span>
            </a>
          ))}
        </div>
        <div class="settings-nav-foot">
          <span class="settings-hint">{SETTINGS_HINT}</span>
          <button
            type="button"
            class="settings-back"
            disabled={status === null || state.finished || savesInFlight(state)}
            aria-describedby={waiting === null ? undefined : "settings-waiting"}
            onClick={() => void controller.leave()}
          >
            {BACK_TO_TALKING}
          </button>
          {waiting === null ? (
            <Problem text={state.problems.start} />
          ) : (
            <p id="settings-waiting" class="setup-problem" role="status">
              {waiting}
            </p>
          )}
        </div>
      </nav>
      <div class="settings-main" ref={main} onScroll={followScroll}>
        <div class="settings-column">
          <header class="settings-head">
            <h1 class="settings-title">{SETTINGS_TITLE}</h1>
            <p class="settings-lede">{SETTINGS_LEDE}</p>
          </header>
          {status === null ? (
            <Problem text={state.loadProblem} />
          ) : (
            <>
              <ThinkingSection
                state={state}
                status={status}
                controller={controller}
                catalog={catalog}
                onAddKey={addKey}
              />
              <VoiceSection
                state={state}
                status={status}
                controller={controller}
                onAddKey={addKey}
              />
              <MicrophoneSection state={state} status={status} controller={controller} />
              <KeysSection
                state={state}
                status={status}
                controller={controller}
                openKey={openKey}
                onOpenKey={setOpenKey}
              />
              <div class="settings-pair">
                <CodingSection state={state} status={status} controller={controller} />
                <MemorySection state={state} status={status} controller={controller} />
              </div>
            </>
          )}
        </div>
      </div>
    </main>
  );
}
