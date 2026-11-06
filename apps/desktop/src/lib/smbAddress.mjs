// The SMB form's one address line (components/SmbDialog.tsx): server, share
// and folder in one, as a file app or Windows shows it – plain JavaScript
// without imports, so the node tests (apps/desktop/tests/smbAddress.test.mjs)
// run it as it is; the types live in smbAddress.d.mts. The backend checks
// the parts again (`SmbConfig::share` in src-tauri/src/remote/smb.rs).

/**
 * The address's parts: `\\nas\Daten\Keystead`, `smb://nas/Daten/Keystead`,
 * `//nas/Daten` or plain `192.168.1.10/Daten/Keystead` (also with a port,
 * `nas:4455/Daten`). A user in front of the server (`smb://ich@nas/…`) is
 * returned apart. `null` while the server or the share is still missing,
 * and for a PC drive (`Z:\Daten`, see `isDriveAddress`).
 */
export function parseSmbAddress(text) {
  if (isDriveAddress(text)) return null;
  let rest = String(text ?? "").trim();
  let url = false;
  for (const prefix of ["smb://", "\\\\", "//"]) {
    if (rest.slice(0, prefix.length).toLowerCase() === prefix) {
      rest = rest.slice(prefix.length);
      url = prefix === "smb://";
      break;
    }
  }
  const parts = rest
    .split(/[\\/]+/)
    .map((part) => part.trim())
    .filter((part) => part !== "")
    .map((part) => (url ? decoded(part) : part));
  if (parts.length < 2) return null;
  let server = parts[0];
  let user = "";
  const at = server.lastIndexOf("@");
  if (at >= 0) {
    user = server.slice(0, at);
    server = server.slice(at + 1);
  }
  if (server === "") return null;
  return { server, share: parts[1], folder: parts.slice(2).join("/"), user };
}

/**
 * A Windows drive letter (`Z:\Daten\Keystead`, `Z:/…`, `Z:`): the PC's name
 * for a share it mounted, which the phone cannot resolve – the form asks for
 * the network path behind it instead. `nas:4455/…` (a port) is no drive.
 */
export function isDriveAddress(text) {
  return /^[a-z]:([\\/]|$)/i.test(String(text ?? "").trim());
}

/** The address line for a linked share (to change it): `nas/Daten/Keystead`. */
export function formatSmbAddress(share) {
  return [share.server, share.share, ...String(share.folder ?? "").split("/")]
    .map((part) => String(part ?? "").trim())
    .filter((part) => part !== "")
    .join("/");
}

// `%20` and friends in an smb:// URL; a broken escape stays as typed.
function decoded(part) {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
}
