import { useId } from "react";

/** Shield with the keyhole cut out (even-odd), drawn on the 512 × 512 grid of assets/keystead.svg. */
const SHIELD_WITH_KEYHOLE =
  "M256 100 388 150v104c0 82-56 136-132 164-76-28-132-82-132-164V150ZM242.61 265.06a32 32 0 1 1 26.78 0L278 336h-44Z";

/**
 * The Keystead mark.
 * - `full` (default): white shield with keyhole on the blue→violet rounded tile (assets/keystead.svg).
 * - `glyph`: only the shield in `currentColor`, for single-colour places (assets/keystead-glyph.svg).
 */
export function Logo({
  size = 32,
  title,
  variant = "full",
}: {
  size?: number;
  title?: string;
  variant?: "full" | "glyph";
}) {
  // Gradient ids must be unique per instance: a reference to a gradient inside a hidden SVG does not render.
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const a11y = {
    role: title ? "img" : undefined,
    "aria-hidden": title ? undefined : true,
    "aria-label": title,
  } as const;

  if (variant === "glyph") {
    return (
      <svg className="logo logo-glyph" width={size} height={size} viewBox="0 0 512 512" {...a11y}>
        <path fill="currentColor" fillRule="evenodd" d={SHIELD_WITH_KEYHOLE} />
      </svg>
    );
  }

  const bg = `ks-bg-${uid}`;
  const hl = `ks-hl-${uid}`;
  return (
    <svg className="logo" width={size} height={size} viewBox="0 0 512 512" {...a11y}>
      <defs>
        <linearGradient id={bg} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#2f6fed" />
          <stop offset="1" stopColor="#7c4dff" />
        </linearGradient>
        <linearGradient id={hl} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity="0.22" />
          <stop offset="0.55" stopColor="#fff" stopOpacity="0" />
        </linearGradient>
      </defs>
      <rect width="512" height="512" rx="116" fill={`url(#${bg})`} />
      <rect width="512" height="512" rx="116" fill={`url(#${hl})`} />
      <path fill="#fff" fillRule="evenodd" d={SHIELD_WITH_KEYHOLE} />
    </svg>
  );
}
