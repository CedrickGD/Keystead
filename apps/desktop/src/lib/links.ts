// Links to the GitHub project (opened in the system browser).

const REPO_URL = "https://github.com/CedrickGD/Keystead";

/** Versions CI builds and publishes (`2.0.0`, `2.0.0-beta.57`, `2.1.0-rc.1`). */
const RELEASE_VERSION = /^\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+)?$/;

/**
 * The browser extension ZIP of the running version's release
 * (`…/releases/download/v<version>/Keystead-<version>-browser-extension.zip`);
 * development builds have no such release and get the releases page.
 */
export function extensionDownloadUrl(version: string): string {
  if (import.meta.env.DEV || !RELEASE_VERSION.test(version)) return `${REPO_URL}/releases`;
  return `${REPO_URL}/releases/download/v${version}/Keystead-${version}-browser-extension.zip`;
}
