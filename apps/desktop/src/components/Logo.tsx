/** The VaultX mark: a shield with a keyhole on a rounded tile. */
export function Logo({ size = 32, title }: { size?: number; title?: string }) {
  return (
    <svg
      className="logo"
      width={size}
      height={size}
      viewBox="0 0 32 32"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      <rect className="tile" width="32" height="32" rx="8" />
      <path className="shield" d="M16 5.75 24 8.6v6.35c0 5.1-3.4 9.2-8 10.95-4.6-1.75-8-5.85-8-10.95V8.6z" />
      <circle className="hole" cx="16" cy="14" r="2.4" />
      <path className="hole" d="M14.85 15.3h2.3l.65 4.7h-3.6z" />
    </svg>
  );
}
