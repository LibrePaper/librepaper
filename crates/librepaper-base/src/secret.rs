//! A value that must not reach a log, a panic message or a debug print.
//!
//! The only ways out of a `Secret` are `expose()` and the wrapped type's own
//! impls. There is no `Display` and no `Serialize`. Dropping it overwrites
//! the value with zeroes first.

use std::fmt;

use zeroize::Zeroize;

pub struct Secret<T: Zeroize>(T);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Secret<T> {
        Secret(value)
    }

    /// The wrapped value. Call it where the value is used, and nowhere a
    /// copy would outlive the call.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Secret<T> {
        Secret(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_prints_redacted_and_never_the_value() {
        let secret = Secret::new("hunter2".to_string());
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(format!("{:?}", Some(&secret)), "Some([redacted])");
    }

    #[test]
    fn expose_returns_the_value_and_clone_copies_it() {
        let secret = Secret::new(vec![1u8, 2, 3]);
        assert_eq!(secret.expose(), &vec![1u8, 2, 3]);
        assert_eq!(secret.clone().expose(), &vec![1u8, 2, 3]);
    }
}
