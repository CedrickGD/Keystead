import { useCallback, useEffect, useRef, useState } from "react";
import { Copy, History, RefreshCw, Trash2 } from "lucide-react";
import { api } from "../lib/api";
import type { GeneratedPassword, GeneratorOptions } from "../lib/types";
import { useT } from "../i18n";
import { useCopy } from "../state/app";
import { useToast } from "../components/Toasts";
import { useConfirm } from "../components/Confirm";
import { Button } from "../components/Controls";
import { EmptyState } from "../components/EmptyState";
import { PasswordText } from "../components/PasswordText";
import { StrengthMeter } from "../components/StrengthMeter";
import { GeneratorOptionsForm, loadGeneratorOptions, saveGeneratorOptions } from "../components/GeneratorOptionsForm";

export function GeneratorPage() {
  const { t, errorText, formatRelative } = useT();
  const toast = useToast();
  const confirm = useConfirm();
  const copy = useCopy();
  const [options, setOptions] = useState<GeneratorOptions>(loadGeneratorOptions);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [history, setHistory] = useState<GeneratedPassword[] | null>(null);
  const [spinning, setSpinning] = useState(false);
  const requestId = useRef(0);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await api.generatorHistory());
    } catch (err) {
      setHistory([]);
      toast.error(errorText(err));
    }
  }, [errorText, toast]);

  const generate = useCallback(
    async (opts: GeneratorOptions) => {
      const id = ++requestId.current;
      try {
        // Every generated value goes into the (local, encrypted) history.
        const next = await api.generatePassword(opts, true);
        if (id !== requestId.current) return;
        setValue(next);
        setError(null);
        void loadHistory();
      } catch (err) {
        if (id === requestId.current) setError(errorText(err));
      }
    },
    [errorText, loadHistory],
  );

  // Regenerate when options change; debounced so dragging a slider creates one entry.
  useEffect(() => {
    saveGeneratorOptions(options);
    const timer = window.setTimeout(() => void generate(options), 250);
    return () => window.clearTimeout(timer);
  }, [options, generate]);

  const regenerate = () => {
    setSpinning(true);
    window.setTimeout(() => setSpinning(false), 400);
    void generate(options);
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

  const copyLabel = options.kind === "passphrase" ? t("generator.passphrase") : t("field.password");

  return (
    <main className="page" aria-labelledby="generator-title">
      <div className="page-inner wide">
        <header className="page-header">
          <h1 id="generator-title">{t("generator.title")}</h1>
          <p>{t("generator.subtitle")}</p>
        </header>

        <section className="card gen-hero">
          <div className="gen-value selectable" aria-live="polite">
            {error ? <span className="field-error">{error}</span> : value ? <PasswordText value={value} /> : " "}
          </div>
          <div className="gen-hero-footer">
            <div className="gen-hero-strength">
              <StrengthMeter password={value} />
            </div>
            <Button variant="secondary" onClick={regenerate} icon={<RefreshCw className={spinning ? "spin-once" : ""} />}>
              {t("generator.regenerate")}
            </Button>
            <Button variant="primary" onClick={() => void copy(value, { label: copyLabel, sensitive: true })} icon={<Copy />} disabled={!value}>
              {t("common.copy")}
            </Button>
          </div>
        </section>

        <div className="gen-layout">
          <section className="card card-pad gen-options-card" aria-label={t("generator.options")}>
            <h2 className="card-title">{t("generator.options")}</h2>
            <GeneratorOptionsForm options={options} onChange={setOptions} />
          </section>

          <section className="card gen-history" aria-label={t("generator.history")}>
            <div className="gen-history-head">
              <h2 className="card-title">
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
                {(history ?? []).map((entry, idx) => (
                  <li key={`${entry.createdAt}-${idx}`}>
                    <div className="gen-history-main">
                      <PasswordText value={entry.password} className="truncate selectable" />
                      <span className="gen-history-time">{formatRelative(entry.createdAt)}</span>
                    </div>
                    <button
                      type="button"
                      className="icon-btn"
                      onClick={() => void copy(entry.password, { label: t("field.password"), sensitive: true })}
                      title={t("common.copy")}
                      aria-label={t("common.copy")}
                    >
                      <Copy />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <p className="gen-history-note">{t("generator.historyNote")}</p>
          </section>
        </div>
      </div>
    </main>
  );
}
