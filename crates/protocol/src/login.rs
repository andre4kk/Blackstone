//! Lettura dei dati di login: nome, XUID e dispositivo del giocatore.
//! ATTENZIONE: le firme dei JWT NON vengono verificate. Chiunque può dichiararsi chiunque: va bene per lo
//! sviluppo, ma prima di aprire il server al pubblico serve la verifica contro le chiavi di Microsoft/Mojang.
use crate::packets::Login;
use crate::skin::Skin;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;

#[derive(Debug)]
pub struct PlayerInfo {
    pub name: String,
    pub xuid: Option<String>,
    pub uuid: Option<String>,
    pub device_os: Option<i64>,
    pub game_version: Option<String>,
    /// Tipo di autenticazione dichiarato dal client (0 = Xbox Live completa, 2 = autofirmata).
    pub authentication_type: Option<i64>,
    /// Chiavi del Token OpenID, per capire cosa ha mandato il client quando l'XUID non si trova.
    pub token_claim_keys: Vec<String>,
    /// Skin dichiarata nei dati del client; se manca o e' malformata, Steve di ripiego.
    pub skin: Skin,
}

/// Estrae le informazioni del giocatore. In caso di errore ritorna una descrizione di cosa è stato trovato.
pub fn parse(login: &Login) -> Result<PlayerInfo, String> {
    let identity: Value = serde_json::from_str(login.identity).map_err(|e| format!("l'identità non è JSON: {e}"))?;
    let data = claims(login.client_data).ok_or("i dati del client non sono un JWT valido")?;

    // La catena sta in {"chain": [...]} oppure, dal protocollo 944, dentro "Certificate" (come stringa JSON o oggetto).
    let certificate = match identity.get("Certificate") {
        Some(Value::String(s)) => serde_json::from_str(s).ok(),
        other => other.cloned(),
    };
    let chain = certificate.as_ref().unwrap_or(&identity).get("chain").and_then(Value::as_array);
    let extra = chain.into_iter().flatten().filter_map(Value::as_str).filter_map(claims).find_map(|c| c.get("extraData").cloned());

    // Con l'autenticazione Xbox completa l'identità verificata sta nel Token OpenID: `xid` (XUID) e `xname` (gamertag).
    let token = identity.get("Token").and_then(Value::as_str).and_then(claims);

    let text = |v: Option<&Value>, key: &str| v?.get(key)?.as_str().map(str::to_owned);
    let name = text(token.as_ref(), "xname")
        .or_else(|| text(extra.as_ref(), "displayName"))
        .or_else(|| text(Some(&data), "ThirdPartyName"))
        .ok_or_else(|| format!("nome non trovato (chiavi identità: {:?}, chiavi client data: {:?})", keys(&identity), keys(&data)))?;
    Ok(PlayerInfo {
        name,
        xuid: text(token.as_ref(), "xid").or_else(|| text(extra.as_ref(), "XUID")),
        // Login vecchio: `identity` della catena. Login nuovo (Token OpenID): `leguuid` se c'è, altrimenti il client lo ricava
        // dall'XUID come `identityFromXUID` di gophertunnel; con un altro UUID il server scartava i `PlayerSkin` del giocatore.
        uuid: text(extra.as_ref(), "identity")
            .or_else(|| text(token.as_ref(), "leguuid"))
            .or_else(|| text(token.as_ref(), "xid").map(|x| identity_from_xuid(&x))),
        device_os: data.get("DeviceOS").and_then(Value::as_i64),
        game_version: text(Some(&data), "GameVersion"),
        authentication_type: identity.get("AuthenticationType").and_then(Value::as_i64),
        skin: Skin::from_client_data(&data).unwrap_or_else(Skin::fallback),
        token_claim_keys: token.as_ref().map(|t| keys(t).into_iter().map(str::to_owned).collect()).unwrap_or_default(),
    })
}

/// UUID versione 3 da MD5("pocket-auth-1-xuid:" + XUID), in forma testuale: lo stesso che il client usa per sé (gophertunnel `identityFromXUID`).
pub fn identity_from_xuid(xuid: &str) -> String {
    use md5::{Digest, Md5};
    let mut id: [u8; 16] = Md5::new().chain_update(b"pocket-auth-1-xuid:").chain_update(xuid.as_bytes()).finalize().into();
    id[6] = id[6] & 0x0f | 0x30;
    id[8] = id[8] & 0x3f | 0x80;
    let h: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Payload di un JWT (senza verificarne la firma).
fn claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        let b = |v: &str| URL_SAFE_NO_PAD.encode(v);
        format!("{}.{}.{}", b(r#"{"alg":"ES384"}"#), b(&payload.to_string()), b("firma"))
    }

    fn login(identity: &str, client_data: &str) -> PlayerInfo {
        parse(&Login { protocol: 2225, identity, client_data }).unwrap()
    }

    #[test]
    fn certificate_envelope_with_extra_data() {
        let chain = json!({"chain": [jwt(json!({})), jwt(json!({"extraData": {"displayName": "Steve", "XUID": "123", "identity": "uuid-1"}}))]});
        let identity = json!({"AuthenticationType": 0, "Certificate": chain.to_string(), "Token": ""}).to_string();
        let p = login(&identity, &jwt(json!({"ThirdPartyName": "Altro", "DeviceOS": 7, "GameVersion": "1.26.63"})));
        assert_eq!((p.name.as_str(), p.xuid.as_deref(), p.uuid.as_deref()), ("Steve", Some("123"), Some("uuid-1")));
        assert_eq!((p.device_os, p.game_version.as_deref()), (Some(7), Some("1.26.63")));
    }

    #[test]
    fn uuid_from_xuid_matches_the_client() {
        // UUID letto nel PlayerSkin vero del client (19/11/2026) per l'XUID 2535406382079041.
        assert_eq!(identity_from_xuid("2535406382079041"), "b4830f52-6498-3b60-a272-fcafe64230ef");
        // Login nuovo senza `leguuid`: il giocatore prende quello ricavato dall'XUID.
        let identity = json!({"AuthenticationType": 0, "Certificate": "", "Token": jwt(json!({"xid": "2535406382079041", "xname": "Gamer"}))}).to_string();
        assert_eq!(login(&identity, &jwt(json!({}))).uuid.as_deref(), Some("b4830f52-6498-3b60-a272-fcafe64230ef"));
        // con `leguuid` vale quello
        let identity = json!({"AuthenticationType": 0, "Certificate": "", "Token": jwt(json!({"xid": "1", "xname": "G", "leguuid": "abc"}))}).to_string();
        assert_eq!(login(&identity, &jwt(json!({}))).uuid.as_deref(), Some("abc"));
    }

    #[test]
    fn openid_token_carries_xuid_and_gamertag() {
        let identity = json!({"AuthenticationType": 0, "Certificate": "", "Token": jwt(json!({"xid": "2535", "xname": "Gamer", "iss": "x"}))}).to_string();
        let p = login(&identity, &jwt(json!({"ThirdPartyName": "Altro"})));
        assert_eq!((p.name.as_str(), p.xuid.as_deref(), p.authentication_type), ("Gamer", Some("2535"), Some(0)));
        assert_eq!(p.token_claim_keys.len(), 3);
    }

    #[test]
    fn legacy_chain_at_top_level() {
        let identity = json!({"chain": [jwt(json!({"extraData": {"displayName": "Alex"}}))]}).to_string();
        assert_eq!(login(&identity, &jwt(json!({}))).name, "Alex");
    }

    #[test]
    fn falls_back_to_client_data_name() {
        let p = login(r#"{"AuthenticationType":2,"Certificate":"","Token":"x"}"#, &jwt(json!({"ThirdPartyName": "Solo"})));
        assert_eq!((p.name.as_str(), p.xuid), ("Solo", None));
    }

    #[test]
    fn reports_what_it_found_when_name_is_missing() {
        let err = parse(&Login { protocol: 1, identity: r#"{"Foo":1}"#, client_data: &jwt(json!({"Bar": 2})) }).unwrap_err();
        assert!(err.contains("Foo") && err.contains("Bar"), "{err}");
    }

    #[test]
    fn login_without_skin_uses_fallback_and_with_skin_reads_it() {
        use base64::engine::general_purpose::STANDARD;
        let identity = r#"{"AuthenticationType":2,"Certificate":"","Token":"x"}"#;
        assert_eq!(login(identity, &jwt(json!({"ThirdPartyName": "Solo"}))).skin, Skin::fallback());
        let data = json!({"ThirdPartyName": "Solo", "SkinId": "s1", "SkinImageWidth": 1, "SkinImageHeight": 1, "SkinData": STANDARD.encode([1u8, 2, 3, 4])});
        assert_eq!(login(identity, &jwt(data)).skin.id, "s1");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&Login { protocol: 1, identity: "non json", client_data: "x" }).is_err());
        assert!(parse(&Login { protocol: 1, identity: "{}", client_data: "non.un-jwt" }).is_err());
    }
}
