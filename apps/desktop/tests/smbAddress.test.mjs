// node --test apps/desktop/tests/*.test.mjs – the SMB form's address line.

import { test } from "node:test";
import assert from "node:assert/strict";
import { formatSmbAddress, isDriveAddress, parseSmbAddress } from "../src/lib/smbAddress.mjs";

const nas = (folder, user = "") => ({ server: "nas", share: "Daten", folder, user });

test("the ways a file app or Windows writes an address", () => {
  for (const text of [
    "\\\\nas\\Daten\\Keystead",
    "smb://nas/Daten/Keystead",
    "SMB://nas/Daten/Keystead/",
    "//nas/Daten/Keystead",
    "nas/Daten/Keystead",
    "nas\\Daten\\Keystead",
    "  nas / Daten / Keystead  ",
    "nas//Daten///Keystead",
  ]) {
    assert.deepEqual(parseSmbAddress(text), nas("Keystead"), text);
  }
  assert.deepEqual(parseSmbAddress("nas/Daten/Backups/Keystead"), nas("Backups/Keystead"));
  assert.deepEqual(parseSmbAddress("\\\\nas\\Daten"), nas(""));
});

test("ports, IP addresses, users and escapes", () => {
  assert.deepEqual(parseSmbAddress("192.168.1.10:4455/Daten"), {
    server: "192.168.1.10:4455",
    share: "Daten",
    folder: "",
    user: "",
  });
  assert.deepEqual(parseSmbAddress("smb://ich@nas/Daten/Keystead"), nas("Keystead", "ich"));
  assert.deepEqual(parseSmbAddress("smb://nas/Meine%20Daten/Key%20stead"), {
    server: "nas",
    share: "Meine Daten",
    folder: "Key stead",
    user: "",
  });
  // Only an smb:// URL is decoded; a broken escape stays as typed.
  assert.equal(parseSmbAddress("nas/50%25/x").share, "50%25");
  assert.equal(parseSmbAddress("smb://nas/%E0%A4%A/x").share, "%E0%A4%A");
});

test("incomplete: no server or no share yet", () => {
  for (const text of ["", "   ", null, undefined, "nas", "\\\\nas\\", "smb://nas", "smb://", "@/Daten"]) {
    assert.equal(parseSmbAddress(text), null, String(text));
  }
});

test("a PC drive is no address (the form asks for the path behind it)", () => {
  for (const text of ["Z:\\Personal Data\\VaultSync", "z:/Daten", "  Z:", "Z:\\"]) {
    assert.equal(isDriveAddress(text), true, text);
    assert.equal(parseSmbAddress(text), null, text);
  }
  // A port is no drive, nor is a one-letter server with a share.
  for (const text of ["nas:4455/Daten", "a:445/Daten", "z/Daten", "\\\\z\\Daten", "", null]) {
    assert.equal(isDriveAddress(text), false, String(text));
  }
  assert.deepEqual(parseSmbAddress("192.168.2.201/personal_folder/Personal Data/VaultSync"), {
    server: "192.168.2.201",
    share: "personal_folder",
    folder: "Personal Data/VaultSync",
    user: "",
  });
});

test("a linked share back as an address line", () => {
  assert.equal(formatSmbAddress({ server: "nas", share: "Daten", folder: "Backups/Keystead" }), "nas/Daten/Backups/Keystead");
  assert.equal(formatSmbAddress({ server: "nas:4455", share: "Daten", folder: "" }), "nas:4455/Daten");
  assert.equal(formatSmbAddress({ server: "nas", share: "Daten" }), "nas/Daten");
  for (const share of [
    { server: "nas", share: "Daten", folder: "Keystead" },
    { server: "192.168.1.10", share: "Meine Daten", folder: "a/b" },
  ]) {
    assert.deepEqual(parseSmbAddress(formatSmbAddress(share)), { ...share, user: "" });
  }
});
