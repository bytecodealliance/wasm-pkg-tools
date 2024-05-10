#!/usr/bin/env bash

set -eo pipefail

URL="https://github.com/oras-project/oras/releases/download/v1.1.0/oras_1.1.0_linux_amd64.tar.gz"
SHA256="e09e85323b24ccc8209a1506f142e3d481e6e809018537c6b3db979c891e6ad7"

mkdir -p /tmp/ci-bin
cd /tmp/ci-bin
curl -L https://github.com/oras-project/oras/releases/download/v1.1.0/oras_1.1.0_linux_amd64.tar.gz -o oras.tgz
echo "$SHA256 oras.tgz" > oras.sum
sha256sum --check oras.sum
tar xf oras.tgz

echo "Got /tmp/ci-bin/oras"