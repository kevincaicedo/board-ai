// The settings page's first section (artboard 12 · Settings, "Thinking · The talker"): the
// provider, the model table from `setup_models` (D-153), the chosen model's panel, and which
// model writes memory digests (the scribe). Its numbers come only from the app's lists.

import { useState } from "preact/hooks";
import type { KeyName } from "../protocol/generated/KeyName.ts";
import type { ModelProvider } from "../protocol/generated/ModelProvider.ts";
import type { SetupStatus } from "../protocol/generated/SetupStatus.ts";
import type { TalkerProvider } from "../protocol/generated/TalkerProvider.ts";
import { Problem } from "./cards.tsx";
import type { ModelCatalog } from "./catalog.ts";
import { sourceFor } from "./catalog.ts";
import type { SetupController } from "./controller.ts";
import {
  contextLabel,
  latencyLabel,
  modelRows,
  orderNote,
  priceLabel,
  type Source,
  shownRows,
  speedColumns,
  speedLabel,
} from "./models.ts";
import { SectionHead } from "./SettingsSections.tsx";
import {
  CLAUDE_MODEL_NOTE,
  drawsDiagrams,
  hiddenRows,
  MODEL_SEARCH_HINT,
  modelCount,
  NO_MATCH,
  OTHER_PROVIDER_NOTE,
  PROVIDERS_SHOWN_MAX,
  providerNote,
  providerRow,
  scribeChangeable,
  scribeLine,
  servedBy,
  TALKER_PROVIDERS,
  talkerKeyMissing,
  thinkingLabel,
} from "./settingsWords.ts";
import type { SetupState } from "./state.ts";

interface ThinkingProps {
  readonly state: SetupState;
  readonly status: SetupStatus;
  readonly controller: SetupController;
  readonly catalog: ModelCatalog;
  readonly onAddKey: (key: KeyName) => void;
}

function Scribe({ status, controller, catalog, busy }: ThinkingProps & { busy: boolean }) {
  const [choosing, setChoosing] = useState(false);
  const listing = catalog.state.value.listing;
  const models = listing.kind === "loaded" ? listing.models.map((model) => model.id) : [];
  const choices = models.includes(status.scribe_model) ? models : [status.scribe_model, ...models];
  const changeable = scribeChangeable(status.talker.provider) && models.length > 0;
  return (
    <span class="settings-scribe">
      {scribeLine(status)}
      {changeable ? " · " : null}
      {changeable && choosing ? (
        <select
          class="settings-scribe-select"
          aria-label="Model for memory digests"
          value={status.scribe_model}
          disabled={busy}
          onChange={(event) => {
            setChoosing(false);
            void controller.pickScribe(event.currentTarget.value);
          }}
          onBlur={() => setChoosing(false)}
        >
          {choices.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
      ) : null}
      {changeable && !choosing ? (
        <button type="button" class="settings-link-button" onClick={() => setChoosing(true)}>
          change
        </button>
      ) : null}
    </span>
  );
}

function ProviderPills({ status, controller, busy }: ThinkingProps & { busy: boolean }) {
  const current = status.talker.provider;
  const pills: readonly { readonly id: TalkerProvider; readonly label: string }[] =
    current === "Other"
      ? [...TALKER_PROVIDERS, { id: "Other", label: "Other · OpenAI-compatible" }]
      : TALKER_PROVIDERS;
  return (
    <div role="radiogroup" aria-label="Talker provider" class="settings-providers">
      {pills.map((pill) => (
        <label key={pill.id} class="settings-provider">
          <input
            type="radio"
            name="settings-provider"
            class="setup-radio"
            checked={pill.id === current}
            disabled={busy || pill.id === "Other"}
            onChange={() => void controller.pickTalker(pill.id)}
          />
          {pill.label}
          <span class="settings-provider-note">{providerNote(pill.id, status)}</span>
        </label>
      ))}
    </div>
  );
}

function ModelTable({
  status,
  controller,
  catalog,
  busy,
  source,
}: ThinkingProps & { busy: boolean; source: Source }) {
  const view = catalog.state.value;
  const listing = view.listing;
  if (listing.kind === "failed") {
    return (
      <div class="setup-retry">
        <Problem text={listing.reason} />
        <button type="button" class="setup-again" onClick={() => void catalog.retry()}>
          Try again
        </button>
      </div>
    );
  }
  if (listing.kind !== "loaded") {
    return <p class="setup-note">Asking for the list…</p>;
  }
  const selected = status.talker.model;
  const rows = modelRows(listing.models, source, view.providers);
  const shown = shownRows(rows, view.search, selected);
  const speeds = speedColumns(shown.rows);
  const pick = (id: string): void => {
    if (!busy) {
      void controller.pickModel(source, id);
    }
  };
  return (
    <div class="settings-models-block">
      <label for="settings-model-search" class="settings-label">
        {modelCount(listing.models.length, listing.leftOut, source)}
      </label>
      <div class="settings-search">
        <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" class="settings-glass">
          <circle cx="11" cy="11" r="6.5" />
          <path d="M16 16l4.5 4.5" />
        </svg>
        <input
          id="settings-model-search"
          type="search"
          placeholder={MODEL_SEARCH_HINT}
          autocomplete="off"
          spellcheck={false}
          value={view.search}
          onInput={(event) => catalog.search(event.currentTarget.value)}
        />
        <span class="settings-search-note">{orderNote(shown.rows, source)}</span>
      </div>
      <div
        role="listbox"
        aria-label="Models"
        aria-busy={busy}
        class={speeds ? "settings-models settings-models-speeds" : "settings-models"}
      >
        <div class="settings-models-head" aria-hidden="true">
          <span>Model · served by</span>
          {speeds ? <span>First token</span> : null}
          {speeds ? <span>Speed</span> : null}
          <span>Price in / out (per M)</span>
          <span>Context</span>
        </div>
        {shown.rows.map((row) => (
          <div
            key={row.id}
            role="option"
            tabIndex={0}
            aria-selected={row.id === selected}
            class="settings-model-row"
            onClick={() => pick(row.id)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                pick(row.id);
              }
            }}
          >
            <span class="settings-model-cell">
              <span class="settings-model-id">{row.id}</span>
              <span class="settings-model-via">
                {source === "OpenRouter" ? `${row.servedBy} · tools` : row.servedBy}
              </span>
            </span>
            {speeds ? <span>{latencyLabel(row.latencyMs)}</span> : null}
            {speeds ? <span>{speedLabel(row.tokensPerSecond)}</span> : null}
            <span>{priceLabel(row.price)}</span>
            <span>{contextLabel(row.contextTokens)}</span>
          </div>
        ))}
        {shown.rows.length === 0 ? <p class="settings-models-note">{NO_MATCH}</p> : null}
        {shown.hidden > 0 ? <p class="settings-models-note">{hiddenRows(shown.hidden)}</p> : null}
      </div>
    </div>
  );
}

function Providers({ providers, route }: { providers: readonly ModelProvider[]; route: string[] }) {
  if (providers.length === 0) {
    return null;
  }
  const shown = providers.slice(0, PROVIDERS_SHOWN_MAX);
  return (
    <div class="settings-served">
      <span class="settings-mono-label">PROVIDERS · {providers.length}</span>
      <ul class="settings-served-list">
        {shown.map((provider) => {
          const line = providerRow(provider, route);
          return (
            <li key={provider.slug} class={line.routed ? "settings-served-routed" : undefined}>
              <span>{line.name}</span>
              <span class="settings-quiet">{line.detail}</span>
            </li>
          );
        })}
      </ul>
      {providers.length > shown.length ? (
        <span class="setup-note">{providers.length - shown.length} more</span>
      ) : null}
    </div>
  );
}

function Selected({ status, catalog }: ThinkingProps) {
  const providers =
    status.talker.provider === "OpenRouter"
      ? (catalog.state.value.providers.get(status.talker.model) ?? [])
      : [];
  return (
    <aside class="settings-selected" aria-label="Selected model">
      <span class="settings-mono-label">SELECTED</span>
      <span class="settings-selected-id">{status.talker.model}</span>
      <dl class="settings-facts">
        <div>
          <dt>Served by</dt>
          <dd>{servedBy(status, providers)}</dd>
        </div>
        <div>
          <dt>Thinking</dt>
          <dd>{thinkingLabel(status.talker.thinking)}</dd>
        </div>
        <div>
          <dt>Draws diagrams</dt>
          <dd>{drawsDiagrams(status.talker.drawing)}</dd>
        </div>
      </dl>
      <Providers providers={providers} route={status.talker.route} />
    </aside>
  );
}

export function ThinkingSection(props: ThinkingProps) {
  const { state, status, onAddKey } = props;
  const busy = state.busy.has("thinking");
  const provider = status.talker.provider;
  const source = sourceFor(provider);
  const missing = talkerKeyMissing(status);
  return (
    <section
      id="settings-thinking"
      class="settings-section"
      aria-labelledby="settings-thinking-title"
      aria-busy={busy}
    >
      <div class="settings-section-top">
        <SectionHead id="settings-thinking-title" step="THINKING" title="The talker" />
        <Scribe {...props} busy={busy} />
      </div>
      <ProviderPills {...props} busy={busy} />
      {missing === null ? null : (
        <div class="settings-inline">
          <p class="setup-problem">{missing.words}</p>
          <button type="button" class="setup-small" onClick={() => onAddKey(missing.key)}>
            Add the key
          </button>
        </div>
      )}
      <Problem text={state.problems.thinking} />
      <div class="settings-thinking">
        {source === null ? (
          <p class="setup-note">
            {provider === "Claude" ? CLAUDE_MODEL_NOTE : OTHER_PROVIDER_NOTE}
          </p>
        ) : (
          <ModelTable {...props} busy={busy} source={source} />
        )}
        <Selected {...props} />
      </div>
    </section>
  );
}
