//! Overwriting the secrets in copies of vault data that commands hand to
//! the UI. Tauri serializes a command's result and then drops it; wrapped in
//! [`Wiped`], the clone is wiped at that point instead of being freed with
//! the plaintext still in it. (The serialized IPC message itself is owned
//! by Tauri and out of reach.)

use keystead_core::model::{GeneratedPassword, VaultItem};
use serde::{Serialize, Serializer};
use zeroize::Zeroize;

/// A value whose strings can be overwritten in place.
pub trait Wipe {
    fn wipe(&mut self);
}

impl Wipe for VaultItem {
    fn wipe(&mut self) {
        self.name.zeroize();
        self.notes.zeroize();
        if let Some(l) = self.login.as_mut() {
            l.username.zeroize();
            l.password.zeroize();
            l.totp.zeroize();
            for u in &mut l.uris {
                u.uri.zeroize();
            }
        }
        if let Some(c) = self.card.as_mut() {
            for s in [
                &mut c.cardholder_name,
                &mut c.brand,
                &mut c.number,
                &mut c.exp_month,
                &mut c.exp_year,
                &mut c.code,
            ] {
                s.zeroize();
            }
        }
        if let Some(i) = self.identity.as_mut() {
            for s in [
                &mut i.title,
                &mut i.first_name,
                &mut i.last_name,
                &mut i.email,
                &mut i.phone,
                &mut i.company,
                &mut i.address1,
                &mut i.address2,
                &mut i.postal_code,
                &mut i.city,
                &mut i.state,
                &mut i.country,
                &mut i.username,
            ] {
                s.zeroize();
            }
        }
        for f in &mut self.fields {
            f.name.zeroize();
            f.value.zeroize();
        }
        for h in &mut self.password_history {
            h.password.zeroize();
        }
    }
}

impl Wipe for GeneratedPassword {
    fn wipe(&mut self) {
        self.password.zeroize();
    }
}

impl<T: Wipe> Wipe for Vec<T> {
    fn wipe(&mut self) {
        self.iter_mut().for_each(Wipe::wipe);
    }
}

/// A command result that serializes like `T` and is wiped when dropped.
pub struct Wiped<T: Wipe>(pub T);

impl<T: Wipe + Serialize> Serialize for Wiped<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<T: Wipe> Drop for Wiped<T> {
    fn drop(&mut self) {
        self.0.wipe();
    }
}

#[cfg(test)]
mod tests {
    use keystead_core::model::{
        CardData, CustomField, IdentityData, ItemType, LoginData, LoginUri, PasswordHistoryEntry,
    };

    use super::*;

    fn secret_item() -> VaultItem {
        VaultItem {
            id: "id-1".into(),
            item_type: ItemType::Login,
            name: "Bank".into(),
            notes: "backup codes 1234".into(),
            login: Some(LoginData {
                username: "alice".into(),
                password: "hunter2".into(),
                uris: vec![LoginUri {
                    uri: "https://bank.example".into(),
                    ..LoginUri::default()
                }],
                totp: "JBSWY3DPEHPK3PXP".into(),
                password_revised_at: None,
            }),
            card: Some(CardData {
                number: "4111111111111111".into(),
                code: "123".into(),
                ..CardData::default()
            }),
            identity: Some(IdentityData {
                last_name: "Doe".into(),
                ..IdentityData::default()
            }),
            fields: vec![CustomField {
                name: "PIN".into(),
                value: "9876".into(),
                ..CustomField::default()
            }],
            password_history: vec![PasswordHistoryEntry {
                password: "old-pass".into(),
                replaced_at: 1,
            }],
            ..VaultItem::default()
        }
    }

    #[test]
    fn wiped_serializes_like_the_value_and_wipes_every_secret() {
        let items = vec![secret_item()];
        let plain = serde_json::to_string(&items).unwrap();
        let wiped = Wiped(items);
        assert_eq!(serde_json::to_string(&wiped).unwrap(), plain);

        let mut item = secret_item();
        item.wipe();
        let json = serde_json::to_string(&item).unwrap();
        for secret in [
            "Bank",
            "backup codes",
            "alice",
            "hunter2",
            "bank.example",
            "JBSWY3DPEHPK3PXP",
            "4111111111111111",
            "\"123\"",
            "Doe",
            "9876",
            "old-pass",
        ] {
            assert!(!json.contains(secret), "{secret} survived: {json}");
        }
        // Ids and structure stay (no secrets).
        assert_eq!(item.id, "id-1");
        assert_eq!(item.password_history.len(), 1);
    }
}
