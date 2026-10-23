import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import {
  BookmarkPlus,
  Check,
  Copy,
  Eye,
  EyeOff,
  History,
  MoreHorizontal,
  Pencil,
  RefreshCw,
  RotateCcw,
  Save,
  Star,
  Trash2,
  WandSparkles,
} from "lucide-react";
import { api, events, subscribeEffect } from "../lib/api";
import {
  DEFAULT_GENERATOR_OPTIONS,
  type GeneratedPasswordInfo,
  type GeneratorOptions,
  type GeneratorPreset,
  type GeneratorPresets,
} from "../lib/types";
import { GENERATOR_PRESETS_MAX, generatorIssue, lenientOptions, normalizedOptions, sameOptions } from "../lib/generatorRules";
import { hasModalLayer } from "../lib/layers";
import { isMac, localPrefs } from "../lib/utils";
import { useT } from "../i18n";
import { useCopy } from "../state/app";
import { useToast } from "../components/Toasts";
import { useConfirm } from "../components/Confirm";
import { Button } from "../components/Controls";
import { EmptyState } from "../components/EmptyState";
import { PasswordText } from "../components/PasswordText";
import { CopyButton } from "../components/CopyButton";
import { StrengthMeter } from "../components/StrengthMeter";
import { Menu, MenuItem } from "../components/Menu";
import { GeneratorOptionsForm, enterGenerates } from "../components/GeneratorOptionsForm";
import { PresetNameDialog, PresetSelect, findPreset, isBuiltinPreset, presetName } from "../components/GeneratorPresets";
import { useRevealedGenerated } from "../components/useRevealedSecret";

const PAGE_KEY = "generator.page";
const MASK = "••••••••••••";

interface PageState {
  presetId: string | null;
  options: GeneratorOptions;
}

/**
 * The page's last preset and options on this device – not secret, a
 * convenience like the remembered options before presets existed. Without
 * one the page starts with the default preset.
 */
function loadPageState(): PageState | null {
  const raw = localPrefs.get(PAGE_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as { presetId?: unknown; options?: unknown };
    return {
      presetId: typeof parsed.presetId === "string" ? parsed.presetId : null,
      options: lenientOptions(parsed.options),
    };
  } catch {
    return null;
  }
}

/**
 * One history entry: masked until the eye is clicked (`reveal_generated`,
 * hidden again after 30 s, on blur, lock and unmount); copy goes through the
 * backend (`copy_generated`) – the password never reaches the page for it.
 */
function HistoryRow({ entry }: { entry: GeneratedPasswordInfo }) {
  const { t, formatRelative, formatDateTime } = useT();
  const secret = useRevealedGenerated(entry.id);
  const shown = secret.value !== null;
  const label = t("generator.historyEntry", { date: formatDateTime(entry.createdAt) });
  const toggleLabel = shown ? t("common.hide") : t("common.show");
  return (
    <li>
      <div className="gen-history-main">
        {secret.value !== null ? (
          <PasswordText value={secret.value} className="gen-history-value selectable" />
        ) : (
          <span className="mask" aria-label={label}>
            {MASK}
          </span>
        )}
        <span className="gen-history-time" title={formatDateTime(entry.createdAt)}>
          {formatRelative(entry.createdAt)}
        </span>
      </div>
      <button
        type="button"
        className="icon-btn"
        onClick={secret.toggle}
        aria-busy={secret.loading}
        title={toggleLabel}
        aria-label={`${toggleLabel}: ${label}`}
        aria-pressed={shown}
      >
        {shown ? <EyeOff /> : <Eye />}
      </button>
      <CopyButton generatedId={entry.id} label={t("field.password")} />
    </li>
  );
}

type NameDialog = { mode: "new" } | { mode: "rename"; preset: GeneratorPreset };

export function GeneratorPage() {
  const { t, errorText } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const copy = useCopy();
  const presetSelectId = useId();
  const stored = useMemo(loadPageState, []);
  const [presets, setPresets] = useState<GeneratorPresets | null>(null);
  const [presetId, setPresetId] = useState<string | null>(stored?.presetId ?? null);
  const [options, setOptions] = useState<GeneratorOptions>(stored?.options ?? { ...DEFAULT_GENERATOR_OPTIONS });
  const [ready, setReady] = useState(false);
  // The one generator value the page holds: a preview the user asked for, not stored.
  const [preview, setPreview] = useState<{ value: string; options: GeneratorOptions } | null>(null);
  const [genError, setGenError] = useState<string | null>(null);
  const [spinning, setSpinning] = useState(false);
  const [copied, setCopied] = useState(false);
  const [history, setHistory] = useState<GeneratedPasswordInfo[] | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [nameDialog, setNameDialog] = useState<NameDialog | null>(null);
  const pageRef = useRef<HTMLElement>(null);
  const generateRef = useRef<HTMLButtonElement>(null);
  const menuAnchor = useRef<HTMLButtonElement>(null);
  const requestId = useRef(0);
  const copiedTimer = useRef(0);
  const spinTimer = useRef(0);
  useEffect(
    () => () => {
      window.clearTimeout(copiedTimer.current);
      window.clearTimeout(spinTimer.current);
    },
    [],
  );

  // Presets once on mount: without a remembered state the page starts with the default preset.
  const loadedPresets = useRef(false);
  useEffect(() => {
    if (loadedPresets.current) return;
    loadedPresets.current = true;
    api
      .generatorPresets()
      .then((loaded) => {
        setPresets(loaded);
        if (!stored) {
          setPresetId(loaded.defaultId);
          setOptions({ ...loaded.defaultOptions });
        } else if (stored.presetId !== null && !findPreset(loaded, stored.presetId)) {
          // The remembered preset was deleted meanwhile: keep its options as custom settings.
          setPresetId(null);
        }
      })
      .catch((err) => toast.error(errorText(err)))
      .finally(() => setReady(true));
  }, [stored, errorText, toast]);

  useEffect(() => {
    if (ready) localPrefs.set(PAGE_KEY, JSON.stringify({ presetId, options }));
  }, [ready, presetId, options]);

  // Focus the primary action, so Enter generates right away.
  useEffect(() => {
    if (ready) generateRef.current?.focus({ preventScroll: true });
  }, [ready]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await api.generatorHistory());
    } catch (err) {
      setHistory([]);
      toast.error(errorText(err));
    }
  }, [errorText, toast]);

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);
  // Another process (e.g. the terminal UI) saved the vault: its history may have changed.
  useEffect(() => subscribeEffect(events.onVaultChanged(() => void loadHistory())), [loadHistory]);

  const issue = generatorIssue(options);
  const stale = preview !== null && !sameOptions(preview.options, options);

  // Only on the user's request – never on open or on an option change.
  const generate = async () => {
    if (!ready || issue) return;
    const id = ++requestId.current;
    const opts = options;
    setSpinning(true);
    window.clearTimeout(spinTimer.current);
    spinTimer.current = window.setTimeout(() => setSpinning(false), 400);
    try {
      const value = await api.generatePreview(opts);
      if (id !== requestId.current) return;
      setPreview({ value, options: opts });
      setGenError(null);
      setCopied(false);
    } catch (err) {
      if (id === requestId.current) setGenError(errorText(err));
    }
  };
  const generateLatest = useRef(generate);
  generateLatest.current = generate;

  // Enter / Ctrl+G while the focus is on this page (not in a dialog).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey || hasModalLayer()) return;
      const active = document.activeElement;
      const onPage = !active || active === document.body || Boolean(pageRef.current?.contains(active));
      if (!onPage) return;
      const mod = e.ctrlKey || e.metaKey;
      const shortcut = mod && !e.shiftKey && e.key.toLowerCase() === "g";
      const enter = e.key === "Enter" && !mod && !e.shiftKey && enterGenerates(active);
      if (!shortcut && !enter) return;
      e.preventDefault();
      // After the input's own Enter handler has committed its value.
      window.setTimeout(() => void generateLatest.current(), 0);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const copyLabel = preview?.options.kind === "passphrase" ? t("generator.passphrase") : t("field.password");
  const copyPreview = async () => {
    if (!preview) return;
    const value = preview.value;
    if (!(await copy(value, { label: copyLabel, sensitive: true }))) return;
    setCopied(true);
    window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => setCopied(false), 1500);
    // Copied = used: now it goes into the history (the backend skips a repeat of the newest entry).
    try {
      await api.rememberGenerated(value);
      await loadHistory();
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const clearHistory = async () => {
    const ok = await confirm({
      title: t("generator.clearHistoryTitle"),
      message: t("generator.clearHistoryText"),
      confirmLabel: t("generator.clearHistory"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      await api.clearGeneratorHistory();
      setHistory([]);
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  // ------------------------------------------------------------- presets

  const current = findPreset(presets, presetId);
  const isUser = Boolean(current && !isBuiltinPreset(current.id));
  const dirty = Boolean(current) && !sameOptions(normalizedOptions(options), current?.options ?? options);
  const isDefault = Boolean(current && presets && current.id === presets.defaultId);
  const atLimit = (presets?.user.length ?? 0) >= GENERATOR_PRESETS_MAX;

  const selectPreset = (preset: GeneratorPreset) => {
    setPresetId(preset.id);
    setOptions({ ...preset.options });
  };

  const adopt = (next: GeneratorPresets, id: string) => {
    setPresets(next);
    const saved = findPreset(next, id);
    if (saved) {
      setPresetId(saved.id);
      setOptions({ ...saved.options });
    }
  };

  const saveNew = async (name: string) => {
    const result = await api.saveGeneratorPreset(null, name, options);
    adopt(result.presets, result.id);
    toast.success(t("generator.presetSaved", { name }));
  };

  const saveChanges = async () => {
    if (!current || !isUser) return;
    try {
      const result = await api.saveGeneratorPreset(current.id, current.name, options);
      adopt(result.presets, result.id);
      toast.success(t("generator.presetSaved", { name: current.name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const rename = async (preset: GeneratorPreset, name: string) => {
    // The stored options stay; unsaved changes on the page stay unsaved.
    const result = await api.saveGeneratorPreset(preset.id, name, preset.options);
    setPresets(result.presets);
    toast.success(t("generator.presetRenamed", { name }));
  };

  const remove = async (preset: GeneratorPreset) => {
    const wasDefault = presets?.defaultId === preset.id;
    const name = presetName(t, preset);
    const ok = await confirm({
      title: t("generator.presetDeleteTitle", { name }),
      message: wasDefault
        ? `${t("generator.presetDeleteText")} ${t("generator.presetDeleteDefault")}`
        : t("generator.presetDeleteText"),
      confirmLabel: t("generator.presetDelete"),
      tone: "danger",
    });
    if (!ok) return;
    try {
      const next = await api.deleteGeneratorPreset(preset.id);
      setPresets(next);
      // Its options stay on the page as custom settings.
      if (presetId === preset.id) setPresetId(null);
      toast.success(t("generator.presetDeleted", { name }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const makeDefault = async (preset: GeneratorPreset) => {
    try {
      setPresets(await api.setDefaultGeneratorPreset(preset.id));
      toast.success(t("generator.presetDefaultSet", { name: presetName(t, preset) }));
    } catch (err) {
      toast.error(errorText(err));
    }
  };

  const closeMenu = () => setMenuOpen(false);
  const shortcut = t("generator.shortcut", { mod: isMac() ? "⌘" : "Ctrl" });

  let note: { tone: "error" | "warning" | "info"; text: string } | null = null;
  if (genError) note = { tone: "error", text: genError };
  else if (issue) note = { tone: "warning", text: t("generator.invalid") };
  else if (stale) note = { tone: "info", text: t("generator.stale") };
  else if (preview) note = { tone: "info", text: t("generator.previewNote") };

  return (
    <main className="page" aria-labelledby="generator-title" ref={pageRef}>
      <div className="page-inner wide">
        <header className="page-header">
          <h1 id="generator-title">{t("generator.title")}</h1>
          <p>{t("generator.subtitle")}</p>
        </header>

        <section className="card gen-hero" aria-label={t("generator.generate")}>
          {preview ? (
            <div className="gen-value selectable" aria-live="polite">
              <PasswordText value={preview.value} />
            </div>
          ) : (
            <div className="gen-empty">
              <span className="gen-empty-icon" aria-hidden>
                <WandSparkles />
              </span>
              <div>
                <div className="gen-empty-title">{t("generator.emptyTitle")}</div>
                <p className="gen-empty-hint">{t("generator.emptyHint")}</p>
              </div>
            </div>
          )}
          {note && (
            <p className={`gen-note ${note.tone}`} role={note.tone === "error" ? "alert" : undefined}>
              {note.text}
            </p>
          )}
          <div className="gen-hero-footer">
            <div className="gen-hero-strength">
              {preview ? <StrengthMeter password={preview.value} /> : <span className="gen-shortcut">{shortcut}</span>}
            </div>
            <Button
              ref={generateRef}
              variant={preview ? "secondary" : "primary"}
              onClick={() => void generate()}
              disabled={!ready || Boolean(issue)}
              title={shortcut}
              icon={preview ? <RefreshCw className={spinning ? "spin-once" : ""} /> : <WandSparkles />}
            >
              {preview ? t("generator.regenerate") : t("generator.generateShort")}
            </Button>
            {preview && (
              <Button
                variant="primary"
                className="gen-copy"
                onClick={() => void copyPreview()}
                icon={copied ? <Check /> : <Copy />}
              >
                {copied ? t("common.copied") : t("common.copy")}
              </Button>
            )}
          </div>
        </section>

        <div className="gen-layout">
          <section className="card card-pad gen-options-card" aria-labelledby="gen-options-title">
            <h2 className="card-title" id="gen-options-title">
              {t("generator.options")}
            </h2>

            <div className="gen-preset">
              <label className="field-label" htmlFor={presetSelectId}>
                {t("generator.preset")}
              </label>
              <div className="gen-preset-row">
                <PresetSelect
                  id={presetSelectId}
                  presets={presets}
                  value={presetId}
                  onSelect={selectPreset}
                  className="gen-preset-select"
                />
                <div className="popover-anchor">
                  <button
                    ref={menuAnchor}
                    type="button"
                    className="icon-btn"
                    onClick={() => setMenuOpen((open) => !open)}
                    aria-haspopup="menu"
                    aria-expanded={menuOpen}
                    aria-label={t("generator.presetActions")}
                    title={t("generator.presetActions")}
                    disabled={!presets}
                  >
                    <MoreHorizontal />
                  </button>
                  {menuOpen && (
                    <Menu onClose={closeMenu} anchorRef={menuAnchor} align="right" label={t("generator.presetActions")} width={250}>
                      <MenuItem
                        icon={<BookmarkPlus />}
                        disabled={Boolean(issue) || atLimit}
                        hint={atLimit ? <span className="menu-hint">{t("generator.presetsLimit")}</span> : undefined}
                        onSelect={() => {
                          closeMenu();
                          setNameDialog({ mode: "new" });
                        }}
                      >
                        {t("generator.presetSaveAs")}
                      </MenuItem>
                      {current && isUser && dirty && (
                        <MenuItem
                          icon={<Save />}
                          disabled={Boolean(issue)}
                          onSelect={() => {
                            closeMenu();
                            void saveChanges();
                          }}
                        >
                          {t("generator.presetSaveChanges")}
                        </MenuItem>
                      )}
                      {current && isUser && (
                        <MenuItem
                          icon={<Pencil />}
                          onSelect={() => {
                            closeMenu();
                            setNameDialog({ mode: "rename", preset: current });
                          }}
                        >
                          {t("generator.presetRename")}
                        </MenuItem>
                      )}
                      {current && !isDefault && (
                        <MenuItem
                          icon={<Star />}
                          onSelect={() => {
                            closeMenu();
                            void makeDefault(current);
                          }}
                        >
                          {t("generator.presetMakeDefault")}
                        </MenuItem>
                      )}
                      {current && isUser && (
                        <MenuItem
                          icon={<Trash2 />}
                          danger
                          onSelect={() => {
                            closeMenu();
                            void remove(current);
                          }}
                        >
                          {t("generator.presetDelete")}
                        </MenuItem>
                      )}
                    </Menu>
                  )}
                </div>
              </div>
              {current && (
                <div className="gen-preset-status">
                  {dirty ? (
                    <>
                      <span className="chip">{t("generator.presetChanged")}</span>
                      <button type="button" className="link-btn" onClick={() => setOptions({ ...current.options })}>
                        <RotateCcw size={13} />
                        {t("generator.presetRevert")}
                      </button>
                      {isUser && !issue && (
                        <button type="button" className="link-btn" onClick={() => void saveChanges()}>
                          <Save size={13} />
                          {t("generator.presetSaveChanges")}
                        </button>
                      )}
                    </>
                  ) : isDefault ? (
                    <span className="gen-default" title={t("generator.presetDefaultHint")}>
                      <Star size={13} aria-hidden />
                      {t("generator.presetIsDefault")}
                    </span>
                  ) : (
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() => void makeDefault(current)}
                      title={t("generator.presetDefaultHint")}
                    >
                      <Star size={13} />
                      {t("generator.presetMakeDefault")}
                    </button>
                  )}
                </div>
              )}
            </div>

            <GeneratorOptionsForm options={options} onChange={setOptions} />
          </section>

          <section className="card gen-history" aria-labelledby="gen-history-title">
            <div className="gen-history-head">
              <h2 className="card-title" id="gen-history-title">
                <History size={16} />
                {t("generator.history")}
              </h2>
              {history && history.length > 0 && (
                <button type="button" className="btn btn-ghost btn-sm" onClick={() => void clearHistory()}>
                  <Trash2 />
                  {t("generator.clearHistory")}
                </button>
              )}
            </div>
            {history && history.length === 0 ? (
              <EmptyState icon={<History />} title={t("generator.historyEmpty")} hint={t("generator.historyEmptyHint")} />
            ) : (
              <ul className="gen-history-list">
                {(history ?? []).map((entry) => (
                  <HistoryRow key={entry.id} entry={entry} />
                ))}
              </ul>
            )}
            <p className="gen-history-note">{t("generator.historyNote")}</p>
          </section>
        </div>
      </div>

      {nameDialog?.mode === "new" && (
        <PresetNameDialog
          title={t("generator.presetSaveTitle")}
          text={t("generator.presetSaveText")}
          initial=""
          onSubmit={saveNew}
          onClose={() => setNameDialog(null)}
        />
      )}
      {nameDialog?.mode === "rename" && (
        <PresetNameDialog
          title={t("generator.presetRenameTitle")}
          initial={nameDialog.preset.name}
          onSubmit={(name) => rename(nameDialog.preset, name)}
          onClose={() => setNameDialog(null)}
        />
      )}
    </main>
  );
}
