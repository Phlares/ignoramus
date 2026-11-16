#!/bin/sh
# Writes local.tsr: an RFC 3161 reply over local.txt from a throwaway local authority.
# Run once by hand with OpenSSL 3; commit local.tsr only. The keys and certificates stay
# in the temporary folder, which the script prints and does not remove.
# The reply names no TSA, as large public authorities do, so the parser must take the
# authority from the signer certificate.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
cd "$work"
cat > tsa.cnf <<'EOF'
[ tsa ]
default_tsa = tsa_config
[ tsa_config ]
serial = ./serial
signer_cert = ./tsa.crt
signer_key = ./tsa.key
signer_digest = sha256
default_policy = 1.2.3.4.1
digests = sha256
accuracy = secs:1
ordering = no
tsa_name = no
ess_cert_id_chain = no
ess_cert_id_alg = sha256
[ tsa_ext ]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,timeStamping
EOF
echo 01 > serial
key="-newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes"
openssl req -x509 $key -keyout ca.key -out ca.crt -subj "/CN=ignorarium test root" -days 3650
openssl req $key -keyout tsa.key -out tsa.csr -subj "/CN=ignorarium test authority"
openssl x509 -req -in tsa.csr -CA ca.crt -CAkey ca.key -set_serial 2 -days 3650 \
  -extfile tsa.cnf -extensions tsa_ext -out tsa.crt
openssl ts -query -data "$here/local.txt" -sha256 -cert -out query.tsq
openssl ts -reply -config tsa.cnf -queryfile query.tsq -out "$here/local.tsr"
openssl ts -verify -in "$here/local.tsr" -data "$here/local.txt" -CAfile ca.crt -untrusted tsa.crt
echo "keys left in $work"
