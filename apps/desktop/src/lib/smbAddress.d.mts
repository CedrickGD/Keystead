// Types of smbAddress.mjs (see there).

export interface SmbAddress {
  server: string;
  share: string;
  /** Below the share, `/`-separated; `""` = the share itself. */
  folder: string;
  /** A user written in front of the server (`smb://ich@nas/…`), else `""`. */
  user: string;
}

export declare function parseSmbAddress(text: string | null | undefined): SmbAddress | null;

export declare function formatSmbAddress(share: { server: string; share: string; folder?: string | null }): string;
