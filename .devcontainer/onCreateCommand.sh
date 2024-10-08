#!/bin/sh

set -eu

for dir in ~/.cargo ~/.pulumi ~/.cache; do
    sudo chown "${USER}:${USER}" "$dir"
done

(
  cd pulumi
  # Download the version of node which is specified in `package.json`
  npx nvm-auto
  sudo -i "$(which corepack)" enable
  pnpm install --frozen-lockfile
) < /dev/null
