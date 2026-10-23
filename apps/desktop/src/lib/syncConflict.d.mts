// Types of syncConflict.mjs (see there).

export type ConflictDevice = "pc" | "phone";
export type ConflictKeep = "local" | "remote";

export declare function conflictSides(platform: string | null | undefined): {
  local: ConflictDevice;
  remote: ConflictDevice;
};

export interface ConflictChoice {
  device: ConflictDevice;
  keep: ConflictKeep;
}

export declare function conflictChoices(platform: string | null | undefined, passwordChanged: boolean): ConflictChoice[];

export declare function attachmentWarnings(
  platform: string | null | undefined,
  conflict: { lostAttachments?: number; remoteOnlyAttachments?: number },
  choices: ConflictChoice[],
): { pcOnly: number; phoneOnly: number };
