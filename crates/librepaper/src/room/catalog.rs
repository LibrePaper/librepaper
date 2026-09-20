use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::WriteError;
use crate::storage::postgres::MutationAuthorization;

pub(super) fn mutation_authorization(
    actor: &crate::document::store::MutationActor,
) -> Result<MutationAuthorization, WriteError> {
    let account_id = if actor.account_id.is_empty() {
        None
    } else {
        Some(
            Uuid::parse_str(&actor.account_id)
                .map_err(|_| WriteError::Invalid("actor account is invalid".into()))?,
        )
    };
    let session_generation = if actor.session_generation.is_empty() {
        None
    } else {
        Some(
            actor
                .session_generation
                .parse::<i64>()
                .map_err(|_| WriteError::Invalid("actor session is invalid".into()))?,
        )
    };
    let token_hash = if actor.link_hash.is_empty() {
        None
    } else {
        let bytes = hex::decode(&actor.link_hash)
            .map_err(|_| WriteError::Invalid("share link is invalid".into()))?;
        Some(
            bytes
                .try_into()
                .map_err(|_| WriteError::Invalid("share link is invalid".into()))?,
        )
    };
    Ok(MutationAuthorization {
        principal_key: account_id
            .map(|id| id.to_string())
            .or_else(|| token_hash.map(|hash| format!("link:{}", hex::encode(hash))))
            .unwrap_or_else(|| format!("visitor:{}", actor.owner_key)),
        account_id,
        session_generation,
        token_hash,
        policy_editor: actor.policy_editor,
    })
}

pub(super) fn request_digest(value: &Value) -> String {
    fn canonical(value: &Value, output: &mut String) {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(v) => output.push_str(if *v { "true" } else { "false" }),
            Value::Number(v) => output.push_str(&v.to_string()),
            Value::String(v) => output.push_str(&serde_json::to_string(v).unwrap_or_default()),
            Value::Array(values) => {
                output.push('[');
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    canonical(v, output);
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    canonical(&Value::String(key.clone()), output);
                    output.push(':');
                    canonical(&values[key], output);
                }
                output.push('}');
            }
        }
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes);
    hex::encode(Sha256::digest(bytes))
}
