// Settings over the live stage (LLD §3.8): ⌘, or a spoken "open settings" (`OpenSettings` from
// the core) shows the settings page (artboard 12, SettingsPage.tsx) through the first run's
// entry point, which picks the page from the status. Closing it (Esc, ⌘, or "Back to talking")
// finishes setup again: the app closes the page, and restarts the session when something was
// saved. The stage keeps running underneath until then, but its keys wait (`settingsOpen`).

import { effect } from "@preact/signals";
import { render } from "preact";
import { opensSettings } from "../app/keys.ts";
import type { SessionStore } from "../app/session.ts";
import { type FirstRunView, showFirstRun } from "./FirstRun.tsx";
import type { SetupTransport } from "./transport.ts";

/** Listens for ⌘, and spoken requests; returns the uninstaller. */
export function installSettings(
  store: SessionStore,
  transport: SetupTransport,
  view: FirstRunView,
): () => void {
  let host: HTMLElement | null = null;
  const open = (): void => {
    if (host !== null) {
      return;
    }
    const element = document.createElement("div");
    document.body.append(element);
    host = element;
    store.settingsOpen.value = true;
    showFirstRun(element, transport, null, view, () => {
      render(null, element);
      element.remove();
      host = null;
      store.settingsOpen.value = false;
    });
  };
  const keydown = (event: KeyboardEvent): void => {
    if (opensSettings({ code: event.code, meta: event.metaKey, repeat: event.repeat })) {
      event.preventDefault();
      open();
    }
  };
  window.addEventListener("keydown", keydown);
  let heard = store.settingsRequests.peek();
  const stop = effect(() => {
    const requests = store.settingsRequests.value;
    if (requests > heard) {
      heard = requests;
      open();
    }
  });
  return () => {
    window.removeEventListener("keydown", keydown);
    stop();
  };
}
