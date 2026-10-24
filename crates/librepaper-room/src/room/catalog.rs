use serde_json::Value;

pub(super) fn request_digest(value: &Value) -> String {
    librepaper_base::canonical_json::sha256_hex(value)
}
