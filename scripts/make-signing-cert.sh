#!/usr/bin/env bash
# Create the self-signed code-signing certificate WhisperFree is signed with.
#
# Why this exists: an ad-hoc (linker-signed) build has no stable identity, so its
# designated requirement is nothing but the cdhash of the executable. macOS TCC keys
# the Accessibility and Microphone grants to that requirement, so *every* rebuild and
# *every* shipped update silently revokes them — while System Settings goes on showing
# the stale entry switched on. Signing with a fixed certificate makes the requirement
# name the certificate instead of the bytes, and the grants survive.
#
# Losing this certificate is unrecoverable in the same way losing the update key is:
# a release signed with a different one is a different identity to macOS, and every
# installed copy drops its permissions on the next update. Back up the .p12.
#
# Usage: scripts/make-signing-cert.sh [output-dir]   (default ~/.whisperfree-signing)

set -euo pipefail

OUT_DIR="${1:-$HOME/.whisperfree-signing}"
COMMON_NAME="WhisperFree Code Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if [ -e "$OUT_DIR/cert.p12" ]; then
  echo "error: $OUT_DIR/cert.p12 already exists." >&2
  echo "Refusing to overwrite it — a new certificate is a new identity, and would" >&2
  echo "cost every installed copy its permissions. Delete it deliberately first." >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"

PASSWORD="$(openssl rand -base64 24)"

echo "==> generating a 10-year code-signing certificate"
openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
  -keyout "$OUT_DIR/key.pem" -out "$OUT_DIR/cert.pem" \
  -subj "/CN=$COMMON_NAME/O=WhisperFree" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

# macOS's Security framework cannot read OpenSSL 3's default PBE/MAC algorithms, so
# the bundle has to be written with the legacy ones or `security import` fails with
# "MAC verification failed".
echo "==> packaging as .p12"
openssl pkcs12 -export -out "$OUT_DIR/cert.p12" \
  -inkey "$OUT_DIR/key.pem" -in "$OUT_DIR/cert.pem" \
  -name "$COMMON_NAME" \
  -macalg sha1 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES \
  -passout "pass:$PASSWORD" 2>/dev/null

printf '%s' "$PASSWORD" > "$OUT_DIR/password.txt"

# The .p12 already holds the key and the certificate together, so the loose PEM key
# is a second, *unencrypted* copy of the only thing here worth stealing. Remove it:
# what is left is a password-encrypted bundle, a public certificate, and a password
# that belongs somewhere other than this directory.
rm -f "$OUT_DIR/key.pem"
chmod 600 "$OUT_DIR"/*

echo "==> importing into the login keychain"
security import "$OUT_DIR/cert.p12" -k "$KEYCHAIN" -P "$PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security >/dev/null

# Without this, codesign cannot read the private key non-interactively and every
# build stops on a "codesign wants to access key" dialog instead — including builds
# run from CI-shaped scripts, where there is nobody to click it. It needs the login
# keychain password, which is why it is asked for rather than guessed; an empty one
# fails silently and leaves you with the dialog on every single build.
echo "==> allowing codesign to use the key without prompting"
printf '    login keychain password (usually your macOS account password): '
read -r -s KEYCHAIN_PASSWORD
echo
if security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
     -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "    done"
else
  echo "    warning: could not set the partition list."
  echo "    Builds will show a 'codesign wants to access key' dialog — click"
  echo "    \"Always Allow\" (not \"Allow\") the first time and it will stop asking."
fi
unset KEYCHAIN_PASSWORD

# codesign refuses an identity it does not trust (CSSMERR_TP_NOT_TRUSTED), so the
# certificate has to be marked as a code-signing anchor. This prompts for your login
# password, and is the only interactive step.
echo "==> trusting it for code signing (macOS will ask for your password)"
security add-trusted-cert -p codeSign -k "$KEYCHAIN" "$OUT_DIR/cert.pem"

echo "==> verifying"
if security find-identity -v -p codesigning | grep -qF "$COMMON_NAME"; then
  echo "    identity is valid and usable"
else
  echo "error: the identity is still not valid for code signing." >&2
  exit 1
fi

cat <<EOF

Done. The certificate lives in $OUT_DIR.

  1. Add this to .env so local builds sign:

     APPLE_SIGNING_IDENTITY="$COMMON_NAME"

  2. Back up $OUT_DIR/cert.p12 and its password somewhere durable, and keep the
     two apart — beside the .p12, the encryption on the bundle protects nothing.
     This certificate cannot be regenerated: losing it costs every installed copy
     its permissions on the next update.

  3. For CI, set these two repository secrets:

     APPLE_CERTIFICATE           $(base64 < "$OUT_DIR/cert.p12" | tr -d '\n' | cut -c1-24)...  (full value below)
     APPLE_CERTIFICATE_PASSWORD  (contents of $OUT_DIR/password.txt)

     Full APPLE_CERTIFICATE value:

$(base64 < "$OUT_DIR/cert.p12" | tr -d '\n')

EOF
