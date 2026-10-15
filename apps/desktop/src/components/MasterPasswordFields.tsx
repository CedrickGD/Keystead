import { useId } from "react";
import { useT } from "../i18n";
import type { Strength } from "../lib/types";
import { Field, PasswordInput } from "./Controls";
import { StrengthMeter, useStrength } from "./StrengthMeter";

export const MIN_MASTER_LENGTH = 8;

export interface MasterPasswordState {
  password: string;
  confirm: string;
}

/** Returns an error message key if the new master password is not acceptable. */
export function masterPasswordProblem(
  state: MasterPasswordState,
  strength: Strength | null,
): "master.tooShort" | "master.tooWeak" | "master.mismatch" | "master.required" | null {
  if (!state.password) return "master.required";
  if (state.password.length < MIN_MASTER_LENGTH) return "master.tooShort";
  if (strength && strength.score < 2) return "master.tooWeak";
  if (state.password !== state.confirm) return "master.mismatch";
  return null;
}

/** New master password + confirmation with a live strength meter. */
export function MasterPasswordFields({
  value,
  onChange,
  showErrors,
  strength,
  labels,
  autoFocus,
}: {
  value: MasterPasswordState;
  onChange: (value: MasterPasswordState) => void;
  showErrors: boolean;
  strength: Strength | null;
  labels?: { password?: string; confirm?: string };
  autoFocus?: boolean;
}) {
  const { t } = useT();
  const pwId = useId();
  const confirmId = useId();
  const problem = masterPasswordProblem(value, strength);
  const pwError =
    showErrors && (problem === "master.required" || problem === "master.tooShort" || problem === "master.tooWeak")
      ? t(problem, { min: MIN_MASTER_LENGTH })
      : null;
  const confirmError = showErrors && problem === "master.mismatch" ? t(problem) : null;
  return (
    <>
      <Field label={labels?.password ?? t("master.new")} htmlFor={pwId} error={pwError}>
        <PasswordInput
          id={pwId}
          value={value.password}
          onChange={(password) => onChange({ ...value, password })}
          invalid={Boolean(pwError)}
          autoFocus={autoFocus}
          size="lg"
        />
        <StrengthMeter password={value.password} strength={strength} emptyHint={t("master.hint", { min: MIN_MASTER_LENGTH })} />
      </Field>
      <Field label={labels?.confirm ?? t("master.confirm")} htmlFor={confirmId} error={confirmError}>
        <PasswordInput
          id={confirmId}
          value={value.confirm}
          onChange={(confirm) => onChange({ ...value, confirm })}
          invalid={Boolean(confirmError)}
          size="lg"
        />
      </Field>
    </>
  );
}

export { useStrength };
