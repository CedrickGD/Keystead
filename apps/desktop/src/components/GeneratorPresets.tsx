import { useId, useState } from "react";
import { ChevronDown } from "lucide-react";
import { BUILTIN_PRESET_IDS, type GeneratorPreset, type GeneratorPresets } from "../lib/types";
import { PRESET_NAME_MAX_CHARS } from "../lib/generatorRules";
import { useT, type I18n } from "../i18n";
import { classNames } from "../lib/utils";
import { Button, Field } from "./Controls";
import { Modal } from "./Modal";

/** True for the id of a built-in preset. */
export function isBuiltinPreset(id: string): boolean {
  return (BUILTIN_PRESET_IDS as readonly string[]).includes(id);
}

/** The shown name: built-in presets are translated by id, user presets keep theirs. */
export function presetName(t: I18n["t"], preset: GeneratorPreset): string {
  switch (preset.id) {
    case "standard":
    case "alnum":
    case "pin":
    case "long":
    case "passphrase":
      return t(`generator.preset.${preset.id}`);
    default:
      return preset.name;
  }
}

/** Preset `id` (built-in or user), if any. */
export function findPreset(presets: GeneratorPresets | null, id: string | null): GeneratorPreset | undefined {
  if (!presets || id === null) return undefined;
  return presets.builtin.find((p) => p.id === id) ?? presets.user.find((p) => p.id === id);
}

/**
 * The preset picker (built-in presets, then the user's). Choosing one loads
 * its options – nothing is generated. `value` null = options that belong to
 * no preset ("Eigene Einstellungen"). The default preset carries a star.
 */
export function PresetSelect({
  presets,
  value,
  onSelect,
  id,
  size = "md",
  className,
}: {
  presets: GeneratorPresets | null;
  value: string | null;
  onSelect: (preset: GeneratorPreset) => void;
  id?: string;
  size?: "sm" | "md";
  className?: string;
}) {
  const { t } = useT();
  const label = (preset: GeneratorPreset) =>
    preset.id === presets?.defaultId ? `${presetName(t, preset)} ★` : presetName(t, preset);
  return (
    <div className={classNames("select-wrap", size === "sm" && "sm", className)}>
      <select
        id={id}
        className="input"
        value={value ?? ""}
        disabled={!presets}
        aria-label={id ? undefined : t("generator.preset")}
        onChange={(e) => {
          const preset = findPreset(presets, e.target.value);
          if (preset) onSelect(preset);
        }}
      >
        {value === null && (
          <option value="" disabled>
            {t("generator.presetCustom")}
          </option>
        )}
        {presets && (
          <optgroup label={t("generator.presetGroupBuiltin")}>
            {presets.builtin.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {label(preset)}
              </option>
            ))}
          </optgroup>
        )}
        {presets && presets.user.length > 0 && (
          <optgroup label={t("generator.presetGroupUser")}>
            {presets.user.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {label(preset)}
              </option>
            ))}
          </optgroup>
        )}
      </select>
      <ChevronDown aria-hidden />
    </div>
  );
}

/**
 * Asks for a preset name (new preset or rename). `onSubmit` saves it; its
 * error (e.g. `preset_name_taken`) is shown below the input.
 */
export function PresetNameDialog({
  title,
  text,
  initial,
  onSubmit,
  onClose,
}: {
  title: string;
  text?: string;
  initial: string;
  onSubmit: (name: string) => Promise<void>;
  onClose: () => void;
}) {
  const { t, errorText } = useT();
  const id = useId();
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const tooLong = Array.from(name.trim()).length > PRESET_NAME_MAX_CHARS;

  const submit = async () => {
    if (busy) return;
    if (!name.trim()) {
      setError(t("error.input.preset_name_required"));
      return;
    }
    if (tooLong) {
      setError(t("error.input.preset_name_too_long"));
      return;
    }
    setBusy(true);
    try {
      await onSubmit(name.trim());
      onClose();
    } catch (err) {
      // Shown below the input (e.g. preset_name_taken); the dialog stays open.
      setError(errorText(err));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={title}
      subtitle={text}
      onClose={onClose}
      onSubmit={() => void submit()}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field label={t("generator.presetName")} htmlFor={id} error={error ?? (tooLong ? t("error.input.preset_name_too_long") : null)}>
        <input
          id={id}
          className={`input ${error || tooLong ? "invalid" : ""}`}
          value={name}
          placeholder={t("generator.presetNamePlaceholder")}
          maxLength={PRESET_NAME_MAX_CHARS + 20}
          autoComplete="off"
          onChange={(e) => {
            setName(e.target.value);
            setError(null);
          }}
          data-autofocus
        />
      </Field>
    </Modal>
  );
}
