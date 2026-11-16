//! Reads an RFC 3161 time-stamp reply. It does not check the authority's signature.
use der::asn1::{AnyRef, GeneralizedTime, IntRef, ObjectIdentifier, OctetStringRef, SetOfVec};
use der::{Decode, Encode, Length, Reader, Sequence, Tag, TagNumber, Tagged, ValueOrd, Writer};
use std::fmt::Write as _;
use std::time::Duration;

pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
const SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
/// GeneralName's `directoryName` alternative, `[4]`; it wraps a Name, so it is constructed.
const DIRECTORY_NAME: Tag = Tag::ContextSpecific {
    constructed: true,
    number: TagNumber(4),
};

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

/// RFC 3161 TimeStampResp.
#[derive(Sequence)]
struct TimeStampResp<'a> {
    status: PkiStatusInfo<'a>,
    #[asn1(optional = "true")]
    token: Option<ContentInfo<'a>>,
}

/// RFC 3161 PKIStatusInfo. `statusString` and `failInfo` are not read.
#[derive(Sequence)]
struct PkiStatusInfo<'a> {
    status: u8,
    rest: Rest<'a>,
}

/// RFC 5652 ContentInfo.
#[derive(Sequence)]
struct ContentInfo<'a> {
    content_type: ObjectIdentifier,
    #[asn1(context_specific = "0")]
    content: AnyRef<'a>,
}

/// RFC 5652 SignedData. `version`, `digestAlgorithms` and `crls` are not read. The
/// certificate and CRL sets read as lists, so a set that lists one certificate twice reads.
#[derive(Sequence)]
struct SignedData<'a> {
    version: AnyRef<'a>,
    digest_algorithms: AnyRef<'a>,
    encap: EncapsulatedContentInfo<'a>,
    #[asn1(context_specific = "0", tag_mode = "IMPLICIT", optional = "true")]
    certificates: Option<Vec<AnyRef<'a>>>,
    #[asn1(context_specific = "1", tag_mode = "IMPLICIT", optional = "true")]
    crls: Option<Vec<AnyRef<'a>>>,
    signer_infos: SetOfVec<AnyRef<'a>>,
}

/// RFC 5652 EncapsulatedContentInfo.
#[derive(Sequence)]
struct EncapsulatedContentInfo<'a> {
    econtent_type: ObjectIdentifier,
    #[asn1(context_specific = "0", optional = "true")]
    econtent: Option<&'a OctetStringRef>,
}

/// RFC 3161 TSTInfo. `version`, `accuracy`, `ordering`, `nonce` and `extensions` are not
/// read; `accuracy`, a SEQUENCE, reads as its elements. `gen_time` is read by `unix_millis`.
#[derive(Sequence)]
struct TstInfo<'a> {
    version: AnyRef<'a>,
    policy: ObjectIdentifier,
    imprint: MessageImprint<'a>,
    serial: IntRef<'a>,
    gen_time: AnyRef<'a>,
    #[asn1(optional = "true")]
    accuracy: Option<Vec<AnyRef<'a>>>,
    #[asn1(optional = "true")]
    ordering: Option<bool>,
    #[asn1(optional = "true")]
    nonce: Option<IntRef<'a>>,
    #[asn1(context_specific = "0", optional = "true")]
    tsa: Option<AnyRef<'a>>,
    extensions: Rest<'a>,
}

/// RFC 3161 MessageImprint.
#[derive(Sequence)]
struct MessageImprint<'a> {
    algorithm: AlgorithmIdentifier<'a>,
    digest: &'a OctetStringRef,
}

/// RFC 5280 AlgorithmIdentifier. `parameters` is not read.
#[derive(Sequence)]
struct AlgorithmIdentifier<'a> {
    oid: ObjectIdentifier,
    #[asn1(optional = "true")]
    parameters: Option<AnyRef<'a>>,
}

/// RFC 5652 SignerInfo up to `sid`: an IssuerAndSerialNumber, or a `[0]` key identifier.
/// `version` and the fields after `sid` are not read.
#[derive(Sequence)]
struct SignerInfo<'a> {
    version: AnyRef<'a>,
    sid: AnyRef<'a>,
    rest: Rest<'a>,
}

/// RFC 5652 IssuerAndSerialNumber.
#[derive(Sequence)]
struct IssuerAndSerialNumber<'a> {
    issuer: Name<'a>,
    serial: IntRef<'a>,
}

/// RFC 5280 Certificate. The signature algorithm and value are not read.
#[derive(Sequence)]
struct Certificate<'a> {
    tbs: TbsCertificate<'a>,
    rest: Rest<'a>,
}

/// RFC 5280 TBSCertificate up to `subject`. `version`, `signature`, `validity` and the
/// fields after `subject` are not read.
#[derive(Sequence)]
struct TbsCertificate<'a> {
    #[asn1(context_specific = "0", optional = "true")]
    version: Option<AnyRef<'a>>,
    serial: IntRef<'a>,
    signature: AnyRef<'a>,
    issuer: Name<'a>,
    validity: AnyRef<'a>,
    subject: Name<'a>,
    rest: Rest<'a>,
}

/// RFC 5280 Name: a SEQUENCE OF RelativeDistinguishedName, each a SET OF these.
type Name<'a> = Vec<SetOfVec<AttributeTypeAndValue<'a>>>;

/// RFC 5280 AttributeTypeAndValue.
#[derive(Sequence, ValueOrd, PartialEq, Eq)]
struct AttributeTypeAndValue<'a> {
    oid: ObjectIdentifier,
    value: AnyRef<'a>,
}

/// The fields left in a SEQUENCE, unread.
struct Rest<'a>(&'a [u8]);

impl<'a> Decode<'a> for Rest<'a> {
    type Error = der::Error;

    fn decode<R: Reader<'a>>(reader: &mut R) -> der::Result<Self> {
        let left = reader.remaining_len();
        reader.read_slice(left).map(Rest)
    }
}

impl Encode for Rest<'_> {
    fn encoded_len(&self) -> der::Result<Length> {
        Length::try_from(self.0.len())
    }

    fn encode(&self, writer: &mut impl Writer) -> der::Result<()> {
        writer.write(self.0)
    }
}

/// A reply that is not granted parses, with empty token fields.
pub fn parse(der: &[u8]) -> Result<Reply, TspError> {
    let resp = TimeStampResp::from_der(der).map_err(bad)?;
    // PKIStatus: granted (0), granted with mods (1), then four kinds of refusal.
    let granted = match resp.status.status {
        0 | 1 => true,
        2..=5 => false,
        n => return Err(bad(format_args!("unknown status {n}"))),
    };
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
    let token = resp.token.ok_or_else(|| bad("granted with no token"))?;
    if token.content_type != SIGNED_DATA {
        return Err(bad("the token is not signed data"));
    }
    let signed: SignedData = token.content.decode_as().map_err(bad)?;
    if signed.encap.econtent_type != TST_INFO {
        return Err(bad("the token does not hold a TSTInfo"));
    }
    let octets = signed.encap.econtent.ok_or_else(|| bad("empty token"))?;
    let tst = TstInfo::from_der(octets.as_bytes()).map_err(bad)?;
    let ms = unix_millis(tst.gen_time)?;
    let named = match tst.tsa {
        Some(tsa) if tsa.tag() == DIRECTORY_NAME => {
            shown(&Name::from_der(tsa.value()).map_err(bad)?)
        }
        _ => None,
    };
    let authority = match named {
        Some(name) => Some(name),
        None => signer_subject(&signed).map_err(bad)?,
    };
    Ok(Reply {
        granted,
        digest_alg: tst.imprint.algorithm.oid.to_string(),
        digest: tst.imprint.digest.as_bytes().to_vec(),
        stamped_at: crate::time::rfc3339_from_unix_millis(ms),
        serial: serial_hex(tst.serial.as_bytes()),
        policy: tst.policy.to_string(),
        authority,
    })
}

/// A GeneralizedTime as Unix milliseconds. DER writes a fraction of a second as `.` and
/// digits with no trailing zero (X.690 11.7); digits past the third are cut.
fn unix_millis(time: AnyRef) -> Result<i64, TspError> {
    time.tag().assert_eq(Tag::GeneralizedTime).map_err(bad)?;
    let text = (time.value().strip_suffix(b"Z")).ok_or_else(|| bad("the time is not in UTC"))?;
    let mut parts = text.splitn(2, |&b| b == b'.');
    let whole = parts.next().unwrap_or_default();
    let mut ms = 0;
    if let Some(fraction) = parts.next() {
        if !fraction.iter().all(u8::is_ascii_digit) || fraction.last().is_none_or(|&d| d == b'0') {
            return Err(bad("the time has a malformed fraction of a second"));
        }
        let digits = fraction.iter().chain(b"00").take(3);
        ms = digits.fold(0, |n, d| n * 10 + u128::from(d - b'0'));
    }
    let whole = [whole, b"Z"].concat();
    let at = AnyRef::new(Tag::GeneralizedTime, &whole)
        .and_then(|t| t.decode_as::<GeneralizedTime>())
        .map_err(bad)?;
    let ms = at.to_unix_duration().as_millis() + ms;
    i64::try_from(ms).map_err(|_| bad("time out of range"))
}

/// Short names of attribute types, as x509-cert 0.2 printed them; each OID as its DER
/// content. Any other type prints as its dotted OID and its value as `#` and DER hex.
const SHORT_NAMES: &[(&[u8], &str)] = &[
    (&[0x55, 0x04, 0x03], "CN"),
    (&[0x55, 0x04, 0x04], "SN"),
    (&[0x55, 0x04, 0x05], "SERIALNUMBER"),
    (&[0x55, 0x04, 0x06], "C"),
    (&[0x55, 0x04, 0x07], "L"),
    (&[0x55, 0x04, 0x08], "ST"),
    (&[0x55, 0x04, 0x09], "STREET"),
    (&[0x55, 0x04, 0x0a], "O"),
    (&[0x55, 0x04, 0x0b], "OU"),
    (&[0x55, 0x04, 0x0c], "TITLE"),
    (&[0x55, 0x04, 0x2a], "GIVENNAME"),
    (
        &[0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x01],
        "UID",
    ),
    (
        &[0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x19],
        "DC",
    ),
    (
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01],
        "EMAIL",
    ),
];

/// `name` as RFC 4514 text: the last RDN first, RDNs joined by `,`, values in one RDN by `+`.
/// `None` when a value does not encode.
fn shown(name: &Name) -> Option<String> {
    let mut s = String::new();
    for (i, rdn) in name.iter().rev().enumerate() {
        if i > 0 {
            s.push(',');
        }
        for (j, atv) in rdn.iter().enumerate() {
            if j > 0 {
                s.push('+');
            }
            let short = SHORT_NAMES
                .iter()
                .find(|(oid, _)| *oid == atv.oid.as_bytes());
            match (short, text(atv.value)) {
                (Some((_, key)), Some(value)) => {
                    s.push_str(key);
                    s.push('=');
                    escape(&mut s, value);
                }
                _ => {
                    let der = atv.value.to_der().ok()?;
                    write!(s, "{}=#{}", atv.oid, hex::encode(der)).ok()?;
                }
            }
        }
    }
    Some(s)
}

/// The value as text when its string type holds text.
fn text(value: AnyRef<'_>) -> Option<&str> {
    let s = std::str::from_utf8(value.value()).ok()?;
    let printable = |b: u8| b.is_ascii_alphanumeric() || b" '()+,-./:=?".contains(&b);
    let fits = match value.tag() {
        Tag::Utf8String => true,
        Tag::PrintableString => s.bytes().all(printable),
        Tag::Ia5String | Tag::TeletexString => s.is_ascii(),
        _ => false,
    };
    fits.then_some(s)
}

/// RFC 4514 escaping of an attribute value.
fn escape(s: &mut String, value: &str) {
    let mut chars = value.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '#' if i == 0 => s.push_str("\\#"),
            ' ' if i == 0 || chars.peek().is_none() => s.push_str("\\ "),
            '"' | '+' | ',' | ';' | '<' | '>' | '\\' => {
                s.push('\\');
                s.push(c);
            }
            '\x00'..='\x1f' | '\x7f' => {
                let _ = write!(s, "\\{:02x}", u32::from(c));
            }
            _ => s.push(c),
        }
    }
}

/// The serial's DER content as lowercase hex, without the `00` that only marks a positive
/// number, as `openssl ts -reply -text` prints it. Zero stays `00`.
fn serial_hex(der: &[u8]) -> String {
    match der {
        [0, rest @ ..] if !rest.is_empty() => hex::encode(rest),
        _ => hex::encode(der),
    }
}

/// The subject of the certificate in the reply that the first signer info names by issuer
/// and serial number.
fn signer_subject(signed: &SignedData) -> der::Result<Option<String>> {
    let Some(first) = signed.signer_infos.get(0) else {
        return Ok(None);
    };
    let sid = first.decode_as::<SignerInfo>()?.sid;
    if sid.tag() != Tag::Sequence {
        return Ok(None);
    }
    let sid: IssuerAndSerialNumber = sid.decode_as()?;
    // The other CertificateChoices alternatives are tagged `[0]` to `[3]`.
    let certificates = signed.certificates.iter().flatten();
    for c in certificates.filter(|c| c.tag() == Tag::Sequence) {
        let tbs = c.decode_as::<Certificate>()?.tbs;
        if tbs.issuer == sid.issuer
            && tbs.serial == sid.serial
            && let Some(subject) = shown(&tbs.subject)
        {
            return Ok(Some(subject));
        }
    }
    Ok(None)
}

/// `text`, `KEY=value` parts joined by `,` with no escapes and each key in `SHORT_NAMES`,
/// as a DER Name of UTF8String values. Other text panics.
fn name_der(text: &str) -> Vec<u8> {
    fn rdn(part: &str) -> SetOfVec<AttributeTypeAndValue<'_>> {
        let (key, value) = part.split_once('=').unwrap();
        let (oid, _) = SHORT_NAMES.iter().find(|(_, k)| *k == key).unwrap();
        let atv = AttributeTypeAndValue {
            oid: ObjectIdentifier::from_bytes(oid).unwrap(),
            value: AnyRef::new(Tag::Utf8String, value.as_bytes()).unwrap(),
        };
        SetOfVec::try_from(vec![atv]).unwrap()
    }
    let name: Name = text.split(',').rev().map(rdn).collect();
    name.to_der().unwrap()
}

/// A granted reply over `digest` at `unix` seconds, with no signer and no certificate. The
/// imprint says SHA-256 at any length of `digest`, so a test can build a bad reply.
/// `authority`, a name such as `CN=T`, goes in the `tsa` field; see `name_der`.
#[doc(hidden)]
pub fn fixture(digest: &[u8], unix: u64, authority: Option<&str>) -> Vec<u8> {
    let time = GeneralizedTime::from_unix_duration(Duration::from_secs(unix)).unwrap();
    let time = time.to_der().unwrap();
    let name = authority.map(name_der);
    let tst = TstInfo {
        version: AnyRef::new(Tag::Integer, &[1]).unwrap(),
        policy: ObjectIdentifier::new_unwrap("1.2.3.4.1"),
        imprint: MessageImprint {
            algorithm: AlgorithmIdentifier {
                oid: ObjectIdentifier::new_unwrap(SHA256),
                parameters: None,
            },
            digest: OctetStringRef::new(digest).unwrap(),
        },
        serial: IntRef::new(&[1]).unwrap(),
        gen_time: AnyRef::from_der(&time).unwrap(),
        accuracy: None,
        ordering: None,
        nonce: None,
        tsa: (name.as_deref()).map(|n| AnyRef::new(DIRECTORY_NAME, n).unwrap()),
        extensions: Rest(&[]),
    };
    let tst = tst.to_der().unwrap();
    let signed = SignedData {
        version: AnyRef::new(Tag::Integer, &[3]).unwrap(),
        digest_algorithms: AnyRef::new(Tag::Set, &[]).unwrap(),
        encap: EncapsulatedContentInfo {
            econtent_type: TST_INFO,
            econtent: Some(OctetStringRef::new(&tst).unwrap()),
        },
        certificates: None,
        crls: None,
        signer_infos: SetOfVec::new(),
    };
    let signed = signed.to_der().unwrap();
    let resp = TimeStampResp {
        status: PkiStatusInfo {
            status: 0,
            rest: Rest(&[]),
        },
        token: Some(ContentInfo {
            content_type: SIGNED_DATA,
            content: AnyRef::from_der(&signed).unwrap(),
        }),
    };
    resp.to_der().unwrap()
}

#[cfg(test)]
mod tests {
    use super::{Name, serial_hex, shown};
    use der::Decode;

    #[test]
    fn the_serial_has_no_sign_byte() {
        assert_eq!(serial_hex(&[0x01]), "01");
        assert_eq!(serial_hex(&[0x00]), "00");
        assert_eq!(serial_hex(&[0x00, 0x9a, 0xbc]), "9abc");
        assert_eq!(serial_hex(&[0x7f, 0x00]), "7f00");
    }

    /// Each DER name beside the text x509-cert 0.2 printed for it.
    #[test]
    fn a_name_prints_as_before() {
        let cases = [
            (
                "3032310b300906035504061302555331173015060355040a0c0e44696769436572742c20496e632e310a300806035504030c0154",
                r"CN=T,O=DigiCert\, Inc.,C=US",
            ),
            (
                "301431123010060b2b0601040182373c0201030c0178",
                "1.3.6.1.4.1.311.60.2.1.3=#0c0178",
            ),
            (
                "30133111300f06092a864886f70d0109011e020041",
                "1.2.840.113549.1.9.1=#1e020041",
            ),
            (
                "30143112301006092a864886f70d0109011303614062",
                "1.2.840.113549.1.9.1=#1303614062",
            ),
            ("30143112301006092a864886f70d0109011403614062", "EMAIL=a@b"),
            ("300c310a300806035504040c0161", "SN=a"),
            ("300c310a30080603550405130161", "SERIALNUMBER=a"),
            ("300c310a300806035504070c0161", "L=a"),
            ("300c310a300806035504080c0161", "ST=a"),
            ("300c310a300806035504090c0161", "STREET=a"),
            ("300c310a3008060355040b0c0161", "OU=a"),
            ("300c310a3008060355040c0c0161", "TITLE=a"),
            ("300c310a3008060355042a0c0161", "GIVENNAME=a"),
            ("30133111300f060a0992268993f22c640119160161", "DC=a"),
            ("30133111300f060a0992268993f22c6401010c0161", "UID=a"),
            ("300d310b3009060355040314024142", "CN=AB"),
            ("3010310e300c06035504030c052361206220", r"CN=\#a b\ "),
            (
                "3011310f300d06035504030c0661222b3b3c3e",
                r#"CN=a\"\+\;\<\>"#,
            ),
            ("300d310b300906035504030c026101", r"CN=a\01"),
            ("300c310a300806035504030c017f", r"CN=\7f"),
            ("300d310b300906035504030c02c3a9", "CN=\u{e9}"),
            (
                "30163114300806035504030c01783008060355040a0c0179",
                "CN=x+O=y",
            ),
            ("300e3100310a300806035504030c0161", "CN=a,"),
            ("3000", ""),
        ];
        for (der, text) in cases {
            let bytes = hex::decode(der).unwrap();
            let name = Name::from_der(&bytes).unwrap();
            assert_eq!(shown(&name).as_deref(), Some(text), "{der}");
        }
    }
}
