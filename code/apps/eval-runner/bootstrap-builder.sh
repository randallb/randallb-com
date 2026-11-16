#!/bin/sh
# Guest-only, verified Nix closure bootstrap. No daemon, SSH or host configuration.
set -eu
test "$(id -u)" = 0
test "$(uname -sm)" = 'Linux aarch64'
nix_closure=$1
ca_bundle=$2
case "$nix_closure" in /nix/store/*-nix-2.34.1) ;; *) exit 1 ;; esac
case "$ca_bundle" in /nix/store/*/etc/ssl/certs/ca-bundle.crt) ;; *) exit 1 ;; esac
mkdir -p /workspace/bootstrap /opt/nix-build /nix/store /nix/var/nix /etc/nix /etc/ssl/certs
chmod 0700 /opt/nix-build
tar -xf /workspace/bootstrap.tar -C /workspace/bootstrap
cp -a /workspace/bootstrap/store/. /nix/store/
if ! getent group nixbld >/dev/null; then
    groupadd -g 30000 nixbld
fi
test "$(getent group nixbld | cut -d: -f3)" = 30000
for index in 1 2 3 4; do
    uid=$((30000 + index))
    if ! id "nixbld$index" >/dev/null 2>&1; then
        useradd -u "$uid" -g nixbld -G nixbld -M -d /var/empty -s /usr/sbin/nologin "nixbld$index"
    fi
    test "$(id -u "nixbld$index")" = "$uid"
    test "$(id -g "nixbld$index")" = 30000
done
chown root:nixbld /nix/store
chmod 1775 /nix/store
cat > /etc/nix/nix.conf <<'EOF'
experimental-features = nix-command flakes
sandbox = true
sandbox-fallback = false
build-users-group = nixbld
max-jobs = 1
cores = 4
build-dir = /opt/nix-build
builders =
EOF
export NIX_REMOTE=local
export NIX_SSL_CERT_FILE="$ca_bundle"
"$nix_closure/bin/nix-store" --load-db < /workspace/bootstrap/reginfo
ln -sfn "$nix_closure" /opt/nix
# Set bundled TLS roots for subsequent exec sessions without inherited host env.
ln -sfn "$ca_bundle" /etc/ssl/certs/ca-certificates.crt
cat >> /etc/nix/nix.conf <<EOF
ssl-cert-file = $ca_bundle
EOF
/opt/nix/bin/nix --version | grep -Fx 'nix (Nix) 2.34.1'
/opt/nix/bin/nix-store --store local --add-root /workspace/nix-bootstrap-root --realise "$nix_closure"
