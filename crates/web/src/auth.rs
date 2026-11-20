//! PIN, sessions and failed-attempt lockout.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Wrong PINs allowed before a lockout.
const MAX_FAILURES: u32 = 5;
const FIRST_LOCKOUT: Duration = Duration::from_secs(30);
const MAX_LOCKOUT: Duration = Duration::from_secs(600);

struct Failures {
    count: u32,
    locked_until: Option<Instant>,
    next_lockout: Duration,
}

pub struct Auth {
    pin: String,
    pin_length: u8,
    sessions: HashSet<String>,
    failures: HashMap<IpAddr, Failures>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginResult {
    /// New session token.
    Ok(String),
    WrongPin {
        attempts_left: u32,
    },
    LockedOut {
        retry_after: Duration,
    },
}

fn random_u64() -> u64 {
    // Falls back to time-based entropy only if the OS RNG fails (never expected).
    getrandom::u64().unwrap_or_else(|_| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    })
}

pub fn new_pin(length: u8) -> String {
    let length = length.clamp(4, 6);
    let max = 10u64.pow(u32::from(length));
    // Rejection sampling avoids modulo bias.
    let zone = u64::MAX - u64::MAX % max;
    loop {
        let v = random_u64();
        if v < zone {
            return format!("{:0width$}", v % max, width = length as usize);
        }
    }
}

fn new_token() -> String {
    format!("{:016x}{:016x}", random_u64(), random_u64())
}

/// Comparison whose time doesn't depend on where the strings differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl Auth {
    pub fn new(pin_length: u8) -> Auth {
        Auth {
            pin: new_pin(pin_length),
            pin_length,
            sessions: HashSet::new(),
            failures: HashMap::new(),
        }
    }

    pub fn pin(&self) -> &str {
        &self.pin
    }

    /// New PIN; every phone has to log in again.
    pub fn regenerate(&mut self) {
        self.pin = new_pin(self.pin_length);
        self.sessions.clear();
        self.failures.clear();
    }

    pub fn is_session(&self, token: &str) -> bool {
        self.sessions.iter().any(|s| same(s, token))
    }

    pub fn logout(&mut self, token: &str) {
        self.sessions.retain(|s| !same(s, token));
    }

    pub fn login(&mut self, ip: IpAddr, pin: &str, now: Instant) -> LoginResult {
        let f = self.failures.entry(ip).or_insert(Failures {
            count: 0,
            locked_until: None,
            next_lockout: FIRST_LOCKOUT,
        });
        if let Some(until) = f.locked_until {
            if now < until {
                return LoginResult::LockedOut {
                    retry_after: until - now,
                };
            }
            f.locked_until = None;
        }
        if same(pin.trim(), &self.pin) {
            self.failures.remove(&ip);
            let token = new_token();
            self.sessions.insert(token.clone());
            return LoginResult::Ok(token);
        }
        f.count += 1;
        if f.count >= MAX_FAILURES {
            f.count = 0;
            f.locked_until = Some(now + f.next_lockout);
            let retry_after = f.next_lockout;
            f.next_lockout = (f.next_lockout * 2).min(MAX_LOCKOUT);
            return LoginResult::LockedOut { retry_after };
        }
        LoginResult::WrongPin {
            attempts_left: MAX_FAILURES - f.count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_have_the_requested_length() {
        for len in [4, 5, 6] {
            for _ in 0..50 {
                let p = new_pin(len);
                assert_eq!(p.len(), len as usize);
                assert!(p.bytes().all(|b| b.is_ascii_digit()));
            }
        }
        assert_eq!(new_pin(2).len(), 4, "clamped to 4..=6");
        assert_eq!(new_pin(9).len(), 6);
    }

    #[test]
    fn login_lockout_and_regenerate() {
        let mut a = Auth::new(4);
        let ip: IpAddr = "192.168.1.50".parse().unwrap();
        let other: IpAddr = "192.168.1.51".parse().unwrap();
        let t0 = Instant::now();
        let wrong = if a.pin() == "0000" { "1111" } else { "0000" };
        for left in (1..5).rev() {
            assert_eq!(
                a.login(ip, wrong, t0),
                LoginResult::WrongPin {
                    attempts_left: left
                }
            );
        }
        assert_eq!(
            a.login(ip, wrong, t0),
            LoginResult::LockedOut {
                retry_after: FIRST_LOCKOUT
            }
        );
        // Even the right PIN is refused while locked out; other devices are unaffected.
        let pin = a.pin().to_owned();
        assert!(matches!(
            a.login(ip, &pin, t0 + Duration::from_secs(10)),
            LoginResult::LockedOut { .. }
        ));
        let LoginResult::Ok(token) = a.login(other, &pin, t0) else {
            panic!("other device")
        };
        assert!(a.is_session(&token));
        // After the lockout, a correct PIN works; the next lockout is longer.
        let LoginResult::Ok(_) = a.login(ip, &pin, t0 + FIRST_LOCKOUT) else {
            panic!("after lockout")
        };
        a.regenerate();
        assert!(!a.is_session(&token), "new PIN logs everyone out");
        assert_ne!(a.pin(), "");
    }

    #[test]
    fn lockout_doubles() {
        let mut a = Auth::new(4);
        let ip: IpAddr = "10.0.0.2".parse().unwrap();
        let wrong = if a.pin() == "0000" { "1111" } else { "0000" };
        let mut now = Instant::now();
        let mut lockouts = Vec::new();
        for _ in 0..3 {
            for _ in 0..4 {
                a.login(ip, wrong, now);
            }
            if let LoginResult::LockedOut { retry_after } = a.login(ip, wrong, now) {
                lockouts.push(retry_after);
                now += retry_after;
            }
        }
        assert_eq!(
            lockouts,
            [
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(120)
            ]
        );
    }
}
