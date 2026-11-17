//! Identità dell'operatore del server (spec NetherNet): chiave P-384 persistente, JWT autofirmato
//! e firma delle fingerprint DTLS da inserire nella risposta SDP.
use base64::{
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64U},
    Engine,
};
use p384::ecdsa::{signature::Signer, Signature, SigningKey};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Identity {
    key: SigningKey,
    jwt: String,
}

impl Identity {
    /// Carica la chiave da `path`, oppure ne genera una nuova e la salva (i giocatori si fidano di questa chiave).
    pub fn load(path: &str) -> Self {
        let key = std::fs::read(path).ok().and_then(|b| SigningKey::from_slice(&b).ok()).unwrap_or_else(|| loop {
            let mut b = [0u8; 48];
            getrandom::fill(&mut b).expect("generatore casuale non disponibile");
            if let Ok(k) = SigningKey::from_slice(&b) {
                std::fs::write(path, b).ok();
                break k;
            }
        });
        let p = key.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty": "EC", "crv": "P-384", "x": B64U.encode(p.x().unwrap()), "y": B64U.encode(p.y().unwrap())});
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = json!({"cpk": jwk, "iat": now, "exp": now + 315_360_000, "iss": "bedrock-rs"});
        let signing_input = format!("{}.{}", header(), B64U.encode(claims.to_string()));
        let jwt = format!("{signing_input}.{}", sign(&key, &signing_input));
        Self { key, jwt }
    }

    /// Inserisce `a=identity` nella risposta SDP: JWT + JWS "detached" sulle fingerprint DTLS.
    pub fn sign_answer(&self, sdp: &str) -> String {
        let fingerprints: Vec<_> = sdp
            .lines()
            .filter_map(|l| l.strip_prefix("a=fingerprint:")?.split_once(' '))
            .map(|(algorithm, digest)| json!({"algorithm": algorithm, "digest": digest.trim()}))
            .collect();
        // serde_json ordina le chiavi, quindi il JSON è canonico.
        let signing_input = format!("{}.{}", header(), B64U.encode(json!({"fingerprint": fingerprints}).to_string()));
        let jws = format!("{}..{}", header(), sign(&self.key, &signing_input)); // detached: payload omesso
        let assertion = json!({"token": self.jwt, "fingerprints": jws}).to_string();
        let envelope = json!({"idp": {"domain": "localhost", "protocol": "default"}, "assertion": assertion});
        let line = format!("a=identity:{}\r\n", B64.encode(envelope.to_string()));

        // La riga va a livello di sessione, cioè prima del primo "m=". Si aggiunge anche end-of-candidates (full ICE).
        let (mut out, mut inserted) = (String::new(), false);
        for l in sdp.lines() {
            if l.starts_with("m=") && !inserted {
                out += &line;
                inserted = true;
            }
            out += l;
            out += "\r\n";
        }
        out + "a=end-of-candidates\r\n"
    }
}

fn header() -> String {
    B64U.encode(r#"{"alg":"ES384"}"#)
}

fn sign(key: &SigningKey, input: &str) -> String {
    let sig: Signature = key.sign(input.as_bytes());
    B64U.encode(sig.to_bytes())
}
