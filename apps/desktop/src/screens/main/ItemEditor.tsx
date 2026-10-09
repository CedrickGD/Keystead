import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ChevronDown, Folder as FolderIcon, Plus, Settings2, Star, Trash2, WandSparkles, X } from "lucide-react";
import type { CardData, CustomField, FieldKind, IdentityData, LoginData, UriMatch, VaultItem } from "../../lib/types";
import { CARD_BRANDS, detectCardBrand, hostOf } from "../../lib/utils";
import { useT } from "../../i18n";
import { Avatar } from "../../components/Avatar";
import { Button, Field, PasswordInput, Select, Switch } from "../../components/Controls";
import { GeneratorPopover } from "../../components/GeneratorPopover";
import { StrengthMeter } from "../../components/StrengthMeter";
import { TYPE_ICONS } from "./ItemList";
import type { Folder } from "../../lib/types";

/** "accounts.google.com" → "Google" */
function nameFromUri(uri: string): string {
  const host = hostOf(uri);
  if (!host) return "";
  if (/^[\d.]+$/.test(host) || !host.includes(".")) return host;
  const parts = host.split(".");
  const label = parts.length >= 2 ? parts[parts.length - 2] ?? host : host;
  return label.charAt(0).toUpperCase() + label.slice(1);
}

function EditSection({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="detail-section">
      <div className="section-label">
        <span>{title}</span>
        {aside && <span style={{ marginLeft: "auto" }}>{aside}</span>}
      </div>
      <div className="card form-card">{children}</div>
    </section>
  );
}

function TextField({
  label,
  value,
  onChange,
  placeholder,
  mono,
  type = "text",
  autoFocus,
  onBlur,
  inputMode,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
  type?: string;
  autoFocus?: boolean;
  onBlur?: () => void;
  inputMode?: "text" | "numeric" | "email" | "tel" | "url";
}) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id}>
      <input
        id={id}
        className={`input ${mono ? "mono" : ""}`}
        type={type}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        autoFocus={autoFocus}
        autoComplete="off"
        spellCheck={false}
        inputMode={inputMode}
      />
    </Field>
  );
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

function PasswordField({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const { t } = useT();
  const id = useId();
  const [genOpen, setGenOpen] = useState(false);
  const anchorRef = useRef<HTMLButtonElement>(null);
  return (
    <Field
      label={t("field.password")}
      htmlFor={id}
      aside={
        <button
          ref={anchorRef}
          type="button"
          className="link-btn"
          onClick={() => setGenOpen((o) => !o)}
          aria-expanded={genOpen}
          aria-haspopup="dialog"
          title={t("generator.generate")}
        >
          <WandSparkles size={14} />
          {t("generator.generateShort")}
        </button>
      }
    >
      <div className="popover-anchor">
        <PasswordInput id={id} value={value} onChange={onChange} />
        {genOpen && (
          <GeneratorPopover anchorRef={anchorRef} onClose={() => setGenOpen(false)} onUse={(pw) => onChange(pw)} />
        )}
      </div>
      <StrengthMeter password={value} />
    </Field>
  );
}

function LoginFields({
  login,
  onChange,
  onUriBlur,
}: {
  login: LoginData;
  onChange: (patch: Partial<LoginData>) => void;
  onUriBlur: (uri: string) => void;
}) {
  const { t } = useT();
  const [advanced, setAdvanced] = useState(() => login.uris.some((u) => u.match !== "domain"));
  const matchOptions: { value: UriMatch; label: string }[] = [
    { value: "domain", label: t("match.domain") },
    { value: "host", label: t("match.host") },
    { value: "startsWith", label: t("match.startsWith") },
    { value: "exact", label: t("match.exact") },
    { value: "never", label: t("match.never") },
  ];
  const setUri = (idx: number, patch: Partial<LoginData["uris"][number]>) =>
    onChange({ uris: login.uris.map((u, i) => (i === idx ? { ...u, ...patch } : u)) });

  return (
    <>
      <EditSection title={t("item.credentials")}>
        <TextField label={t("field.username")} value={login.username} onChange={(username) => onChange({ username })} />
        <PasswordField value={login.password} onChange={(password) => onChange({ password })} />
        <TextField
          label={t("field.totpKey")}
          value={login.totp}
          onChange={(totp) => onChange({ totp })}
          placeholder={t("field.totpPlaceholder")}
          mono
        />
      </EditSection>

      <EditSection
        title={t("field.websites")}
        aside={
          <button type="button" className="link-btn" onClick={() => setAdvanced((a) => !a)} aria-expanded={advanced}>
            <Settings2 size={14} />
            {advanced ? t("item.hideMatching") : t("item.showMatching")}
          </button>
        }
      >
        {login.uris.length === 0 && <div className="field-hint">{t("item.noWebsites")}</div>}
        {login.uris.map((u, idx) => (
          <div key={idx} className={`uri-row ${advanced ? "advanced" : ""}`}>
            <input
              className="input"
              value={u.uri}
              placeholder="https://example.com"
              aria-label={t("field.websiteN", { n: idx + 1 })}
              onChange={(e) => setUri(idx, { uri: e.target.value })}
              onBlur={(e) => onUriBlur(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              inputMode="url"
            />
            {advanced && (
              <Select
                value={u.match}
                onChange={(match) => setUri(idx, { match })}
                options={matchOptions}
                ariaLabel={t("item.matchType")}
              />
            )}
            <button
              type="button"
              className="icon-btn danger"
              onClick={() => onChange({ uris: login.uris.filter((_, i) => i !== idx) })}
              aria-label={t("item.removeWebsite")}
              title={t("item.removeWebsite")}
            >
              <X />
            </button>
          </div>
        ))}
        {advanced && <div className="field-hint">{t("item.matchingHint")}</div>}
        <div>
          <button
            type="button"
            className="btn btn-ghost btn-sm add-btn"
            onClick={() => onChange({ uris: [...login.uris, { uri: "", match: "domain" }] })}
          >
            <Plus />
            {t("item.addWebsite")}
          </button>
        </div>
      </EditSection>
    </>
  );
}

// ---------------------------------------------------------------------------
// Card
// ---------------------------------------------------------------------------

function CardFields({ card, onChange }: { card: CardData; onChange: (patch: Partial<CardData>) => void }) {
  const { t } = useT();
  const codeId = useId();
  const monthId = useId();
  const brandOptions = [
    { value: "", label: t("field.brandNone") },
    ...CARD_BRANDS.map((b) => ({ value: b, label: b })),
    ...(card.brand && !CARD_BRANDS.includes(card.brand) ? [{ value: card.brand, label: card.brand }] : []),
  ];
  const months = [
    { value: "", label: t("field.month") },
    ...Array.from({ length: 12 }, (_, i) => {
      const m = String(i + 1).padStart(2, "0");
      return { value: m, label: m };
    }),
  ];
  return (
    <EditSection title={t("item.cardDetails")}>
      <TextField label={t("field.cardholder")} value={card.cardholderName} onChange={(cardholderName) => onChange({ cardholderName })} />
      <TextField
        label={t("field.cardNumber")}
        value={card.number}
        mono
        inputMode="numeric"
        placeholder="1234 5678 9012 3456"
        onChange={(number) => {
          const detected = detectCardBrand(number);
          const previousDetected = detectCardBrand(card.number);
          // Auto-fill the brand unless the user picked a different one by hand.
          const brand = !card.brand || card.brand === previousDetected ? detected || card.brand : card.brand;
          onChange({ number, brand });
        }}
      />
      <div className="form-grid-2">
        <Field label={t("field.brand")}>
          <Select value={card.brand} onChange={(brand) => onChange({ brand })} options={brandOptions} ariaLabel={t("field.brand")} />
        </Field>
        <Field label={t("field.expiry")} htmlFor={monthId}>
          <div className="expiry-row">
            <Select id={monthId} value={card.expMonth} onChange={(expMonth) => onChange({ expMonth })} options={months} ariaLabel={t("field.month")} />
            <input
              className="input mono"
              value={card.expYear}
              placeholder={t("field.yearPlaceholder")}
              inputMode="numeric"
              maxLength={4}
              aria-label={t("field.year")}
              onChange={(e) => onChange({ expYear: e.target.value.replace(/\D/g, "").slice(0, 4) })}
            />
          </div>
        </Field>
      </div>
      <Field label={t("field.securityCode")} htmlFor={codeId}>
        <PasswordInput id={codeId} value={card.code} onChange={(code) => onChange({ code: code.slice(0, 8) })} inputMode="numeric" />
      </Field>
    </EditSection>
  );
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

function IdentityFields({ identity, onChange }: { identity: IdentityData; onChange: (patch: Partial<IdentityData>) => void }) {
  const { t } = useT();
  const titleId = useId();
  const titles = t("identity.titles").split("|");
  const titleOptions = [
    { value: "", label: "–" },
    ...titles.map((x) => ({ value: x, label: x })),
    ...(identity.title && !titles.includes(identity.title) ? [{ value: identity.title, label: identity.title }] : []),
  ];
  const f = (key: keyof IdentityData, label: string, extra?: { type?: string; inputMode?: "email" | "tel" | "numeric" }) => (
    <TextField label={label} value={identity[key]} onChange={(v) => onChange({ [key]: v })} type={extra?.type} inputMode={extra?.inputMode} />
  );
  return (
    <>
      <EditSection title={t("item.personal")}>
        <div className="form-grid-name">
          <Field label={t("field.title")} htmlFor={titleId}>
            <Select id={titleId} value={identity.title} onChange={(title) => onChange({ title })} options={titleOptions} />
          </Field>
          {f("firstName", t("field.firstName"))}
          {f("lastName", t("field.lastName"))}
        </div>
        <div className="form-grid-2">
          {f("company", t("field.company"))}
          {f("username", t("field.username"))}
        </div>
      </EditSection>
      <EditSection title={t("item.contact")}>
        <div className="form-grid-2">
          {f("email", t("field.email"), { type: "email", inputMode: "email" })}
          {f("phone", t("field.phone"), { type: "tel", inputMode: "tel" })}
        </div>
      </EditSection>
      <EditSection title={t("item.address")}>
        {f("address1", t("field.address1"))}
        {f("address2", t("field.address2"))}
        <div className="form-grid-zip">
          {f("postalCode", t("field.postalCode"))}
          {f("city", t("field.city"))}
        </div>
        <div className="form-grid-2">
          {f("state", t("field.state"))}
          {f("country", t("field.country"))}
        </div>
      </EditSection>
    </>
  );
}

// ---------------------------------------------------------------------------
// Custom fields
// ---------------------------------------------------------------------------

function CustomFieldsEditor({ fields, onChange }: { fields: CustomField[]; onChange: (fields: CustomField[]) => void }) {
  const { t } = useT();
  const kinds: { value: FieldKind; label: string }[] = [
    { value: "text", label: t("fieldKind.text") },
    { value: "hidden", label: t("fieldKind.hidden") },
    { value: "boolean", label: t("fieldKind.boolean") },
  ];
  const update = (idx: number, patch: Partial<CustomField>) =>
    onChange(fields.map((f, i) => (i === idx ? { ...f, ...patch } : f)));
  return (
    <EditSection title={t("item.customFields")}>
      {fields.length === 0 && <div className="field-hint">{t("item.noCustomFields")}</div>}
      {fields.map((field, idx) => (
        <div key={idx} className="custom-field-row">
          <input
            className="input"
            value={field.name}
            placeholder={t("item.fieldName")}
            aria-label={t("item.fieldName")}
            onChange={(e) => update(idx, { name: e.target.value })}
          />
          {field.kind === "boolean" ? (
            <div className="custom-bool">
              <Switch
                checked={field.value === "true"}
                onChange={(checked) => update(idx, { value: checked ? "true" : "false" })}
                label={field.name || t("item.value")}
              />
              <span className="muted">{field.value === "true" ? t("common.yes") : t("common.no")}</span>
            </div>
          ) : field.kind === "hidden" ? (
            <PasswordInput value={field.value} onChange={(value) => update(idx, { value })} aria-label={t("item.value")} placeholder={t("item.value")} />
          ) : (
            <input
              className="input"
              value={field.value}
              placeholder={t("item.value")}
              aria-label={t("item.value")}
              onChange={(e) => update(idx, { value: e.target.value })}
            />
          )}
          <Select
            value={field.kind}
            onChange={(kind) =>
              update(idx, {
                kind,
                value: kind === "boolean" ? (field.value === "true" ? "true" : "false") : field.kind === "boolean" ? "" : field.value,
              })
            }
            options={kinds}
            ariaLabel={t("item.fieldType")}
          />
          <button
            type="button"
            className="icon-btn danger"
            onClick={() => onChange(fields.filter((_, i) => i !== idx))}
            aria-label={t("item.removeField")}
            title={t("item.removeField")}
          >
            <X />
          </button>
        </div>
      ))}
      <div>
        <button
          type="button"
          className="btn btn-ghost btn-sm add-btn"
          onClick={() => onChange([...fields, { name: "", value: "", kind: "text" }])}
        >
          <Plus />
          {t("item.addField")}
        </button>
      </div>
    </EditSection>
  );
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

export interface ItemEditorProps {
  draft: VaultItem;
  folders: Folder[];
  isNew: boolean;
  saving: boolean;
  showErrors: boolean;
  onChange: (draft: VaultItem) => void;
  onSave: () => void;
  onCancel: () => void;
  onDelete: () => void;
}

export function ItemEditor({ draft, folders, isNew, saving, showErrors, onChange, onSave, onCancel, onDelete }: ItemEditorProps) {
  const { t } = useT();
  const nameRef = useRef<HTMLInputElement>(null);
  const notesId = useId();
  const folderId = useId();
  const nameMissing = !draft.name.trim();

  useEffect(() => {
    if (isNew) nameRef.current?.focus();
  }, [isNew]);

  useEffect(() => {
    if (showErrors && nameMissing) nameRef.current?.focus();
  }, [showErrors, nameMissing]);

  const set = (patch: Partial<VaultItem>) => onChange({ ...draft, ...patch });
  const folderOptions = [
    { value: "", label: t("folder.none") },
    ...folders.map((f) => ({ value: f.id, label: f.name })),
  ];

  return (
    <form
      className="detail editing"
      onSubmit={(e) => {
        e.preventDefault();
        onSave();
      }}
      noValidate
    >
      <header className="detail-header">
        <Avatar name={draft.name || "?"} type={draft.type} size="lg" />
        <div className="detail-title">
          <input
            ref={nameRef}
            className={`input name-input ${showErrors && nameMissing ? "invalid" : ""}`}
            value={draft.name}
            placeholder={t("item.namePlaceholder")}
            aria-label={t("field.name")}
            aria-invalid={showErrors && nameMissing}
            onChange={(e) => set({ name: e.target.value })}
            maxLength={200}
          />
          <div className="detail-chips">
            <span className="chip">
              {TYPE_ICONS[draft.type]}
              {isNew ? t("item.newOfType", { type: t(`type.${draft.type}`) }) : t(`type.${draft.type}`)}
            </span>
            <label className="chip-select" title={t("field.folder")}>
              <FolderIcon aria-hidden />
              <select id={folderId} value={draft.folderId ?? ""} onChange={(e) => set({ folderId: e.target.value || null })} aria-label={t("field.folder")}>
                {folderOptions.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
              <ChevronDown aria-hidden />
            </label>
            {showErrors && nameMissing && <span className="chip danger">{t("item.nameRequired")}</span>}
          </div>
        </div>
        <div className="detail-actions">
          <button
            type="button"
            className={`icon-btn ${draft.favorite ? "on" : ""}`}
            onClick={() => set({ favorite: !draft.favorite })}
            aria-pressed={draft.favorite}
            title={draft.favorite ? t("item.unfavorite") : t("item.makeFavorite")}
            aria-label={draft.favorite ? t("item.unfavorite") : t("item.makeFavorite")}
          >
            <Star fill={draft.favorite ? "currentColor" : "none"} />
          </button>
        </div>
      </header>

      <div className="detail-body">
        {draft.type === "login" && draft.login && (
          <LoginFields
            login={draft.login}
            onChange={(patch) => draft.login && set({ login: { ...draft.login, ...patch } })}
            onUriBlur={(uri) => {
              if (!draft.name.trim()) {
                const suggestion = nameFromUri(uri);
                if (suggestion) set({ name: suggestion });
              }
            }}
          />
        )}
        {draft.type === "card" && draft.card && (
          <CardFields card={draft.card} onChange={(patch) => draft.card && set({ card: { ...draft.card, ...patch } })} />
        )}
        {draft.type === "identity" && draft.identity && (
          <IdentityFields
            identity={draft.identity}
            onChange={(patch) => draft.identity && set({ identity: { ...draft.identity, ...patch } })}
          />
        )}

        {draft.type === "note" && (
          <EditSection title={t("field.notes")}>
            <textarea
              className="input note-textarea"
              value={draft.notes}
              onChange={(e) => set({ notes: e.target.value })}
              aria-label={t("field.notes")}
              placeholder={t("item.notePlaceholder")}
              rows={10}
            />
          </EditSection>
        )}

        {draft.type !== "note" && (
          <EditSection title={t("field.notes")}>
            <textarea
              id={notesId}
              className="input"
              value={draft.notes}
              onChange={(e) => set({ notes: e.target.value })}
              aria-label={t("field.notes")}
              placeholder={t("item.notesPlaceholder")}
              rows={4}
            />
          </EditSection>
        )}

        <CustomFieldsEditor fields={draft.fields} onChange={(fields) => set({ fields })} />
      </div>

      <footer className="detail-footer">
        {!isNew && (
          <Button
            variant="danger-ghost"
            icon={<Trash2 />}
            onClick={onDelete}
            disabled={saving}
            title={t("item.moveToTrash")}
            aria-label={t("item.moveToTrash")}
          >
            <span className="hide-narrow">{t("item.moveToTrash")}</span>
          </Button>
        )}
        <span className="spacer" />
        <Button variant="secondary" onClick={onCancel} disabled={saving}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" loading={saving} title={`${t("common.save")} (Ctrl+S)`}>
          {t("common.save")}
        </Button>
      </footer>
    </form>
  );
}
