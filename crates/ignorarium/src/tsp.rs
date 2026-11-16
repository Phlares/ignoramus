//! Reads an RFC 3161 time-stamp reply. It does not check the authority's signature.
use cmpv2::status::{PkiStatus, PkiStatusInfo};
use cms::cert::CertificateChoices;
use cms::cert::x509::der::asn1::{GeneralizedTime, Int, OctetString};
use cms::cert::x509::der::oid::ObjectIdentifier;
use cms::cert::x509::der::{Any, Decode, Encode};
use cms::cert::x509::ext::pkix::name::GeneralName;
use cms::cert::x509::name::Name;
use cms::cert::x509::spki::AlgorithmIdentifier;
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfos};
use std::fmt::Write;
use std::str::FromStr;
use std::time::Duration;
use x509_tsp::{MessageImprint, TimeStampResp, TspVersion, TstInfo};

pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub granted: bool,
    pub digest_alg: String,
    pub digest: Vec<u8>,
    pub stamped_at: String,
    pub serial: String,
    pub policy: String,
    pub authority: Option<String>,
}

#[derive(Debug)]
pub struct TspError(String);

impl std::fmt::Display for TspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a time-stamp reply: {}", self.0)
    }
}

impl std::error::Error for TspError {}

fn bad(what: impl std::fmt::Display) -> TspError {
    TspError(what.to_string())
}

/// A reply that is not granted parses, with empty token fields.
pub fn parse(der: &[u8]) -> Result<Reply, TspError> {
    let resp = TimeStampResp::from_der(der).map_err(bad)?;
    let granted = matches!(
        resp.status.status,
        PkiStatus::Accepted | PkiStatus::GrantedWithMods
    );
    if !granted {
        return Ok(Reply {
            granted,
            digest_alg: String::new(),
            digest: Vec::new(),
            stamped_at: String::new(),
            serial: String::new(),
            policy: String::new(),
            authority: None,
        });
    }
    let token = resp
        .time_stamp_token
        .ok_or_else(|| bad("granted with no token"))?;
    if token.content_type != SIGNED_DATA {
        return Err(bad("the token is not signed data"));
    }
    let signed: SignedData = token.content.decode_as().map_err(bad)?;
    let encap = &signed.encap_content_info;
    if encap.econtent_type != TST_INFO {
        return Err(bad("the token does not hold a TSTInfo"));
    }
    let econtent = encap.econtent.as_ref().ok_or_else(|| bad("empty token"))?;
    let octets: OctetString = econtent.decode_as().map_err(bad)?;
    let tst = TstInfo::from_der(octets.as_bytes()).map_err(bad)?;
    let ms = i64::try_from(tst.gen_time.to_unix_duration().as_millis())
        .map_err(|_| bad("time out of range"))?;
    let authority = match &tst.tsa {
        Some(GeneralName::DirectoryName(name)) => shown(name),
        _ => None,
    }
    .or_else(|| signer_subject(&signed));
    Ok(Reply {
        granted,
        digest_alg: tst.message_imprint.hash_algorithm.oid.to_string(),
        digest: tst.message_imprint.hashed_message.as_bytes().to_vec(),
        stamped_at: crate::time::rfc3339_from_unix_millis(ms),
        serial: serial_hex(tst.serial_number.as_bytes()),
        policy: tst.policy.to_string(),
        authority,
    })
}

/// A name as text; `None` when its formatter fails.
fn shown(name: &Name) -> Option<String> {
    let mut s = String::new();
    write!(s, "{name}").ok().map(|()| s)
}

/// The serial's DER content as lowercase hex, without the `00` that only marks a positive
/// number, as `openssl ts -reply -text` prints it. Zero stays `00`.
fn serial_hex(der: &[u8]) -> String {
    match der {
        [0, rest @ ..] if !rest.is_empty() => hex::encode(rest),
        _ => hex::encode(der),
    }
}

/// The subject of the certificate in the reply that the first signer info names.
fn signer_subject(signed: &SignedData) -> Option<String> {
    let SignerIdentifier::IssuerAndSerialNumber(sid) = &signed.signer_infos.0.get(0)?.sid else {
        return None;
    };
    signed
        .certificates
        .as_ref()?
        .0
        .iter()
        .find_map(|c| match c {
            CertificateChoices::Certificate(c)
                if c.tbs_certificate.issuer == sid.issuer
                    && c.tbs_certificate.serial_number == sid.serial_number =>
            {
                shown(&c.tbs_certificate.subject)
            }
            _ => None,
        })
}

/// A granted reply over `digest` (SHA-256 imprint; any length, to build a bad reply) at `unix` seconds, with no signer and no
/// certificate. `authority`, a name such as `CN=T`, goes in the `tsa` field.
#[doc(hidden)]
pub fn fixture(digest: &[u8], unix: u64, authority: Option<&str>) -> Vec<u8> {
    let tst = TstInfo {
        version: TspVersion::V1,
        policy: ObjectIdentifier::new_unwrap("1.2.3.4.1"),
        message_imprint: MessageImprint {
            hash_algorithm: AlgorithmIdentifier {
                oid: ObjectIdentifier::new_unwrap(SHA256),
                parameters: None,
            },
            hashed_message: OctetString::new(digest.to_vec()).unwrap(),
        },
        serial_number: Int::new(&[1]).unwrap(),
        gen_time: GeneralizedTime::from_unix_duration(Duration::from_secs(unix)).unwrap(),
        accuracy: None,
        ordering: false,
        nonce: None,
        tsa: authority.map(|a| GeneralName::DirectoryName(Name::from_str(a).unwrap())),
        extensions: None,
    };
    let octets = OctetString::new(tst.to_der().unwrap()).unwrap();
    let signed = SignedData {
        version: CmsVersion::V3,
        digest_algorithms: Default::default(),
        encap_content_info: EncapsulatedContentInfo {
            econtent_type: TST_INFO,
            econtent: Some(Any::encode_from(&octets).unwrap()),
        },
        certificates: None,
        crls: None,
        signer_infos: SignerInfos(Default::default()),
    };
    let resp = TimeStampResp {
        status: PkiStatusInfo {
            status: PkiStatus::Accepted,
            status_string: None,
            fail_info: None,
        },
        time_stamp_token: Some(ContentInfo {
            content_type: SIGNED_DATA,
            content: Any::encode_from(&signed).unwrap(),
        }),
    };
    resp.to_der().unwrap()
}

#[cfg(test)]
mod tests {
    use super::serial_hex;

    #[test]
    fn the_serial_has_no_sign_byte() {
        assert_eq!(serial_hex(&[0x01]), "01");
        assert_eq!(serial_hex(&[0x00]), "00");
        assert_eq!(serial_hex(&[0x00, 0x9a, 0xbc]), "9abc");
        assert_eq!(serial_hex(&[0x7f, 0x00]), "7f00");
    }
}
