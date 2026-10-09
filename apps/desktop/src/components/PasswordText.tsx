/** Renders a password with digits and symbols colour-coded for readability. */
export function PasswordText({ value, className }: { value: string; className?: string }) {
  const parts: { text: string; kind: "l" | "d" | "s" }[] = [];
  for (const ch of value) {
    const kind = /[0-9]/.test(ch) ? "d" : /[\p{L}]/u.test(ch) ? "l" : "s";
    const last = parts[parts.length - 1];
    if (last && last.kind === kind) last.text += ch;
    else parts.push({ text: ch, kind });
  }
  return (
    <span className={`pw-text mono ${className ?? ""}`}>
      {parts.map((part, idx) =>
        part.kind === "l" ? (
          <span key={idx}>{part.text}</span>
        ) : (
          <span key={idx} className={part.kind === "d" ? "pw-digit" : "pw-symbol"}>
            {part.text}
          </span>
        ),
      )}
    </span>
  );
}
