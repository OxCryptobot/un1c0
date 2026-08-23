# GitHub authentication notes

_Last updated: 2026-08-21_

The user reports that the `OxCryptobot` GitHub account already has two SSH keys and requests that no additional key be generated. This is the preferred publication path for the Unico repository.

The sandbox currently contains no usable existing private SSH identity: its `/home/ubuntu/.ssh` inventory showed only `known_hosts` after the temporary Phase 58 recovery keypair was removed. The temporary keypair was not added to GitHub and has been securely deleted.

The connected browser now confirms two existing **read/write authentication keys** on the `OxCryptobot` account:

| Title | Fingerprint |
|---|---|
| `un1c0 sandbox push` | `SHA256:Rroxi5I5LCXXQ7DljRomLlX13T05UJXPaugBlPPRcEM` |
| `un1c0 Phase 50 deployment` | `SHA256:UjF7B6DbWEV2WLs6K32xqqoJKCorV2Fp7zQW4Mncq0g` |

The sandbox still has no corresponding private identity or active SSH agent: its `/home/ubuntu/.ssh` inventory contains only `known_hosts`, and no candidate key matched the account fingerprints. The authenticated `gh` CLI can read repository metadata and confirms `ADMIN` permission, but `gh api user/keys` returned `403 Resource not accessible by integration`; the browser inventory above is authoritative for the account-key list.

## Publication rule

Do not create another SSH key unless the user explicitly approves it. Prefer either an existing private key made available in the sandbox or a repository-scoped fine-grained GitHub personal access token with `Contents: Read and write` for `OxCryptobot/un1c0`. Never store, print, or commit a token, private key, or full public-key material in this repository.

## Safe local key check

```bash
find ~/.ssh -maxdepth 1 -type f -printf '%f\n' | sort
for key in ~/.ssh/id_*; do
  [ -f "$key" ] || continue
  case "$key" in *.pub) continue ;; esac
  fingerprint=$(ssh-keygen -y -f "$key" 2>/dev/null | ssh-keygen -lf - 2>/dev/null)
  [ -n "$fingerprint" ] && printf '%s: %s\n' "$(basename "$key")" "$fingerprint"
done
ssh -o BatchMode=yes -o IdentitiesOnly=yes -i ~/.ssh/<candidate-key> -T git@github.com
```

A matching private key must be made available in this sandbox before an SSH push can succeed. The public fingerprint may be recorded for matching; private key contents must never be recorded.
