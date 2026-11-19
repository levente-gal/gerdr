//! Web Push to the phone view (see `phone`): the phones that asked for
//! notifications, and the notes gerdr sends them while the user is away.
//!
//! The phone's browser gives an address at its push service (Apple's,
//! Google's, Mozilla's) and a key; gerdr encrypts each note for that key
//! (RFC 8291) and posts it there, signed with a key of its own (VAPID, RFC
//! 8292), through `curl`. The push service sees only the address and the
//! encrypted note.
//!
//! The phone view keeps the list (`push.json`) and makes the key
//! (`push-key`) when a phone first asks; gerdr only reads them.

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

use aes_gcm::{Aes128Gcm, KeyInit, aead::Aead};
use hkdf::Hkdf;
use p256::{
    PublicKey, SecretKey,
    ecdsa::{Signature, SigningKey, signature::Signer},
    elliptic_curve::sec1::ToEncodedPoint,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// What a failed push is reported as.
const PROBLEM: &str = "phone notifications";
/// How many phones may ask: more is a mistake.
const MOST: usize = 10;
/// How long a push service keeps a note for a phone that's off, in seconds.
const KEEP: u32 = 4 * 3600;

/// A phone that asked for notifications, as its browser described it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    /// Where its push service takes the notes.
    pub endpoint: String,
    /// Its key, base64url: a P-256 point.
    pub p256dh: String,
    /// Its secret, base64url: 16 bytes.
    pub auth: String,
    /// The phone view's address it asked from, like `https://desk.ts.net`:
    /// whom the push service is told the notes come from.
    pub origin: String,
}

/// A note for the phones.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub title: String,
    pub body: String,
    /// A newer note with the same tag replaces this one.
    pub tag: String,
    /// The phone view's page a tap opens, like `/#/f/<project>/<slug>`.
    pub url: String,
}

fn dir() -> PathBuf {
    crate::projects::data_dir().join("phone")
}

fn list_path() -> PathBuf {
    dir().join("push.json")
}

fn key_path() -> PathBuf {
    dir().join("push-key")
}

/// The phones that asked.
pub fn subscriptions() -> Vec<Subscription> {
    std::fs::read_to_string(list_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(list: &[Subscription]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    crate::projects::write_atomic(&list_path(), text.as_bytes()).map_err(|e| e.to_string())
}

/// Add `sub`, or take it off with `on` false. Its keys are checked here:
/// it comes from the network.
pub fn subscribe(sub: Subscription, on: bool) -> Result<(), String> {
    let mut list = subscriptions();
    list.retain(|s| s.endpoint != sub.endpoint);
    if on {
        if !sub.endpoint.starts_with("https://") || !sub.origin.starts_with("https://") {
            return Err("A push address must be https".into());
        }
        let key = b64_decode(&sub.p256dh).ok_or("Bad phone key")?;
        PublicKey::from_sec1_bytes(&key).map_err(|_| "Bad phone key")?;
        if b64_decode(&sub.auth).map(|a| a.len()) != Some(16) {
            return Err("Bad phone secret".into());
        }
        if list.len() >= MOST {
            return Err(format!("{MOST} phones get notifications already"));
        }
        list.push(sub);
    }
    save(&list)
}

/// gerdr's own key's public half, base64url, made the first time.
pub fn public_key() -> Result<String, String> {
    let secret = match secret_key() {
        Some(secret) => secret,
        None => {
            let secret = loop {
                if let Ok(secret) = SecretKey::from_slice(&random::<32>()) {
                    break secret;
                }
            };
            std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
            write_private(&key_path(), b64_encode(&secret.to_bytes()).as_bytes())?;
            secret
        }
    };
    Ok(b64_encode(point(&secret.public_key()).as_bytes()))
}

fn secret_key() -> Option<SecretKey> {
    let text = std::fs::read_to_string(key_path()).ok()?;
    SecretKey::from_slice(&b64_decode(text.trim())?).ok()
}

/// Write `bytes` to `path`, readable by the user only.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `N` random bytes, from the kernel.
fn random<const N: usize>() -> [u8; N] {
    use std::io::Read;
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("/dev/urandom reads");
    bytes
}

fn point(key: &PublicKey) -> p256::EncodedPoint {
    key.to_encoded_point(false)
}

/// Sends notes to the phones, on a thread of its own: each goes through
/// `curl`, which may take seconds.
pub struct Pusher {
    tx: mpsc::Sender<Note>,
}

impl Pusher {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Note>();
        std::thread::spawn(move || {
            // The user is away: what fails waits on the "!" badge.
            for note in rx {
                match send_all(&note) {
                    Ok(_) => crate::problems::clear(PROBLEM),
                    Err(err) => crate::problems::report(
                        PROBLEM,
                        format!("Couldn't notify the phone: {err}"),
                    ),
                }
            }
        });
        Self { tx }
    }

    pub fn send(&self, note: Note) {
        // The thread ends only with gerdr.
        let _ = self.tx.send(note);
    }
}

/// Send `note` to every phone that asked; one its push service says is gone
/// is taken off. How many took it, or why one didn't.
pub fn send_all(note: &Note) -> Result<usize, String> {
    let list = subscriptions();
    if list.is_empty() {
        return Ok(0);
    }
    let secret = secret_key().ok_or_else(|| format!("no key in {}", key_path().display()))?;
    let payload = serde_json::to_vec(note).map_err(|e| e.to_string())?;
    let (mut sent, mut gone, mut failed) = (0, Vec::new(), None);
    for sub in &list {
        match send(sub, &secret, &payload) {
            Ok(200..=202) => sent += 1,
            Ok(404 | 410) => gone.push(sub.endpoint.clone()),
            Ok(code) => failed = Some(format!("{} answered {code}", host(&sub.endpoint))),
            Err(err) => failed = Some(err),
        }
    }
    if !gone.is_empty() {
        let mut list = subscriptions();
        list.retain(|s| !gone.contains(&s.endpoint));
        save(&list)?;
    }
    failed.map_or(Ok(sent), Err)
}

/// Post `payload` to `sub`'s push service; the HTTP status it answered.
fn send(sub: &Subscription, secret: &SecretKey, payload: &[u8]) -> Result<u16, String> {
    let ua = b64_decode(&sub.p256dh).ok_or("bad phone key")?;
    let auth = b64_decode(&sub.auth).ok_or("bad phone secret")?;
    let ephemeral = loop {
        if let Ok(key) = SecretKey::from_slice(&random::<32>()) {
            break key;
        }
    };
    let body = encrypt(&ua, &auth, payload, &ephemeral, random())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let jwt = vapid_jwt(
        secret,
        &audience(&sub.endpoint),
        &sub.origin,
        now + 12 * 3600,
    );
    let public = b64_encode(point(&secret.public_key()).as_bytes());
    let file = crate::phone_control::dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "push-{}-{}.bin",
            std::process::id(),
            u32::from_ne_bytes(random())
        ));
    write_private(&file, &body)?;
    let config = [
        ("url", sub.endpoint.clone()),
        (
            "header",
            format!("Authorization: vapid t={jwt}, k={public}"),
        ),
        ("header", "Content-Encoding: aes128gcm".into()),
        ("header", "Content-Type: application/octet-stream".into()),
        ("header", format!("TTL: {KEEP}")),
        ("header", "Urgency: high".into()),
        ("data-binary", format!("@{}", file.display())),
    ]
    .iter()
    .map(|(k, v)| {
        format!(
            "{k} = \"{}\"\n",
            v.replace('\\', "\\\\").replace('"', "\\\"")
        )
    })
    .collect::<String>();
    let out = curl(&config);
    if let Err(err) = std::fs::remove_file(&file) {
        tracing::warn!("phone push: {}: {err}", file.display());
    }
    let out = out?;
    String::from_utf8_lossy(&out)
        .trim()
        .parse()
        .map_err(|_| "curl gave no status".into())
}

/// Run `curl` with `config` on its stdin (keys stay out of `ps`); what it
/// printed: the status code.
fn curl(config: &str) -> Result<Vec<u8>, String> {
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "15",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
        ])
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "curl is not installed".to_owned(),
            _ => format!("curl: {err}"),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|e| format!("curl: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().next().unwrap_or("curl failed").to_owned());
    }
    Ok(out.stdout)
}

/// `https://host` of `endpoint`: whom the signature is for.
fn audience(endpoint: &str) -> String {
    format!("https://{}", host(endpoint))
}

fn host(endpoint: &str) -> &str {
    let rest = endpoint.split_once("://").map_or(endpoint, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

/// The VAPID token: an ES256 JWT for `aud`, valid until `exp`.
fn vapid_jwt(secret: &SecretKey, aud: &str, sub: &str, exp: u64) -> String {
    let head = b64_encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = serde_json::json!({ "aud": aud, "exp": exp, "sub": sub });
    let signed = format!("{head}.{}", b64_encode(claims.to_string().as_bytes()));
    let signature: Signature = SigningKey::from(secret).sign(signed.as_bytes());
    format!("{signed}.{}", b64_encode(&signature.to_bytes()))
}

/// `payload` encrypted for the phone with key `ua` and secret `auth`, as
/// RFC 8291 says: one aes128gcm record, from `ephemeral` and `salt`.
fn encrypt(
    ua: &[u8],
    auth: &[u8],
    payload: &[u8],
    ephemeral: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>, String> {
    let ua_key = PublicKey::from_sec1_bytes(ua).map_err(|_| "bad phone key")?;
    let shared = p256::ecdh::diffie_hellman(ephemeral.to_nonzero_scalar(), ua_key.as_affine());
    let as_public = point(&ephemeral.public_key());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(ua);
    info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .map_err(|e| e.to_string())?;
    let prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let (mut cek, mut nonce) = ([0; 16], [0; 12]);
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .and_then(|()| prk.expand(b"Content-Encoding: nonce\0", &mut nonce))
        .map_err(|e| e.to_string())?;
    let mut plain = payload.to_vec();
    plain.push(2); // the last record's delimiter
    let sealed = Aes128Gcm::new_from_slice(&cek)
        .map_err(|e| e.to_string())?
        .encrypt(&nonce.into(), plain.as_slice())
        .map_err(|e| e.to_string())?;
    let mut out = salt.to_vec();
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(65);
    out.extend_from_slice(as_public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url, no padding.
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Base64url or base64, padded or not.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut n, mut bits) = (0u32, 0);
    for c in text.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        n = n << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((n >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{VerifyingKey, signature::Verifier};

    fn b(text: &str) -> Vec<u8> {
        b64_decode(text).expect("base64")
    }

    #[test]
    fn base64url_goes_both_ways() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&bytes)), Some(bytes));
        }
        assert_eq!(b64_encode(b"hi?"), "aGk_");
        assert_eq!(b64_decode("aGk/"), Some(b"hi?".to_vec()));
    }

    /// RFC 8291's example, appendix A.
    #[test]
    fn encrypts_as_rfc_8291_says() {
        let ephemeral =
            SecretKey::from_slice(&b("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).expect("key");
        let ua = b(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let auth = b("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = b("DGv6ra1nlYgDCS1FRnbzlw").try_into().expect("salt");
        let body = encrypt(
            &ua,
            &auth,
            b"When I grow up, I want to be a watermelon",
            &ephemeral,
            salt,
        )
        .expect("encrypt");
        assert_eq!(
            b64_encode(&body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn the_token_is_signed_by_the_key() {
        let secret = SecretKey::from_slice(&[7; 32]).expect("key");
        let jwt = vapid_jwt(
            &secret,
            "https://web.push.apple.com",
            "https://desk.ts.net",
            99,
        );
        let (signed, signature) = jwt.rsplit_once('.').expect("three parts");
        let signature = Signature::from_slice(&b(signature)).expect("signature");
        VerifyingKey::from(&secret.public_key())
            .verify(signed.as_bytes(), &signature)
            .expect("verifies");
        let claims: serde_json::Value =
            serde_json::from_slice(&b(signed.split('.').nth(1).expect("claims"))).expect("json");
        assert_eq!(claims["aud"], "https://web.push.apple.com");
        assert_eq!(claims["sub"], "https://desk.ts.net");
        assert_eq!(
            audience("https://fcm.googleapis.com/fcm/send/abc"),
            "https://fcm.googleapis.com"
        );
    }
}
