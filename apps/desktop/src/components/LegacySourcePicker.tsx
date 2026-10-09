import { useEffect, useState } from "react";
import { FileUp } from "lucide-react";
import { api, pickOpenFile } from "../lib/api";
import type { LegacyVaultInfo } from "../lib/types";
import { useT } from "../i18n";

/**
 * Lists the VaultX 1.x vaults found on this computer (`legacy_scan`) as radio
 * options, plus "choose another file". Preselects the first one found.
 */
export function LegacySourcePicker({
  path,
  onPath,
  onError,
  labelledBy,
}: {
  path: string | null;
  onPath: (path: string) => void;
  onError: (message: string) => void;
  labelledBy?: string;
}) {
  const { t, errorText } = useT();
  const [found, setFound] = useState<LegacyVaultInfo[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .legacyScan()
      .then((list) => {
        if (cancelled) return;
        setFound(list);
        if (list[0] && !path) onPath(list[0].path);
      })
      .catch(() => {
        if (!cancelled) setFound([]);
      });
    return () => {
      cancelled = true;
    };
    // Scan once per mount (the preselection must not re-run when `path` changes).
  }, []);

  const pickFile = async () => {
    try {
      const picked = await pickOpenFile({
        title: t("import.pickLegacy"),
        filters: [{ name: "VaultX 1.x", extensions: ["json"] }],
      });
      if (!picked) return;
      setFound((list) => {
        const current = list ?? [];
        if (current.some((v) => v.path === picked)) return current;
        const fileName = picked.split(/[\\/]/).pop() ?? picked;
        return [...current, { name: fileName, path: picked }];
      });
      onPath(picked);
    } catch (err) {
      onError(errorText(err));
    }
  };

  if (found === null) {
    return (
      <div className="legacy-list-loading">
        <span className="spinner" /> {t("import.scanning")}
      </div>
    );
  }

  return (
    <div className="legacy-list" role="radiogroup" aria-labelledby={labelledBy}>
      {found.length === 0 && <div className="legacy-empty">{t("import.noneFound")}</div>}
      {found.map((v) => (
        <label key={v.path} className={`legacy-option ${path === v.path ? "selected" : ""}`}>
          <input type="radio" name="legacy-source" checked={path === v.path} onChange={() => onPath(v.path)} />
          <span className="legacy-option-text">
            <span className="legacy-option-name">{v.name}</span>
            <span className="legacy-option-path" title={v.path}>
              {v.path}
            </span>
          </span>
        </label>
      ))}
      <button type="button" className="legacy-pick" onClick={() => void pickFile()}>
        <FileUp />
        {found.length === 0 ? t("import.chooseFile") : t("import.pickOther")}
      </button>
    </div>
  );
}
