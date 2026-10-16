// A key field (artboard 11, card 1): a password input that saves on paste or Enter and clears
// itself at once. The key is read from the field once and handed to `onSave`; it is never kept
// in state, and once saved the field only says that a key is saved.

import { useRef } from "preact/hooks";
import { KEY_NOTE } from "./words.ts";

export function KeyField({
  id,
  label,
  hint,
  disabled,
  onSave,
}: {
  id: string;
  label: string;
  hint: string;
  disabled: boolean;
  onSave: (value: string) => void;
}) {
  const field = useRef<HTMLInputElement>(null);
  const save = (): void => {
    const input = field.current;
    if (input === null) {
      return;
    }
    const value = input.value;
    input.value = "";
    if (value.trim() !== "") {
      onSave(value);
    }
  };
  return (
    <div class="setup-key">
      <label for={id} class="setup-label">
        {label}
      </label>
      <form
        class="setup-field"
        onSubmit={(event) => {
          event.preventDefault();
          save();
        }}
      >
        <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" class="setup-lock">
          <rect x="5" y="11" width="14" height="10" rx="2" />
          <path d="M8 11V8a4 4 0 0 1 8 0v3" />
        </svg>
        <input
          id={id}
          ref={field}
          type="password"
          placeholder={hint}
          autocomplete="off"
          spellcheck={false}
          disabled={disabled}
          onInput={(event) => {
            if (event instanceof InputEvent && event.inputType === "insertFromPaste") {
              save();
            }
          }}
        />
      </form>
      <span class="setup-note">{KEY_NOTE}</span>
    </div>
  );
}
