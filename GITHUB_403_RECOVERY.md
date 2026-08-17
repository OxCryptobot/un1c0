# GitHub 403 Push Recovery

This guide applies when `git push origin main` returns `403 Permission denied` even though `gh auth status` reports a logged-in account.

> A successful CLI login does not guarantee that the credential Git uses has repository-content write authority.

## 1. Confirm the remote and credential context

Run these commands from the repository. The `GH_TOKEN` environment variable is intentionally removed while testing the stored GitHub CLI credential because environment tokens can override the credential helper.

```bash
git remote -v
env -u GH_TOKEN gh auth status
env -u GH_TOKEN gh repo view OxCryptobot/un1c0 \
  --json nameWithOwner,viewerPermission,isFork
env -u GH_TOKEN git ls-remote origin
```

If `git ls-remote` succeeds but `git push` returns 403, authentication works but the credential is not authorized to write repository contents, the organization requires SAML/SSO approval, or direct pushes to `main` are protected.

## 2. SSH-key path

Use a dedicated key when HTTPS credential precedence is ambiguous. Do not overwrite an existing key and never disclose the private key.

```bash
ssh-keygen -t ed25519 -C "github-un1c0-push" \
  -f ~/.ssh/id_ed25519_un1c0
ssh-add ~/.ssh/id_ed25519_un1c0
gh ssh-key add ~/.ssh/id_ed25519_un1c0.pub \
  --title "un1c0 push key"
git remote set-url origin git@github.com:OxCryptobot/un1c0.git
ssh -T git@github.com
git push origin main
```

If multiple GitHub identities exist, add a host alias to `~/.ssh/config` and use that alias in the remote URL. The expected SSH test result is an authenticated GitHub greeting; GitHub does not provide shell access through that connection.

## 3. Fine-grained PAT path

Create or refresh a fine-grained token with access to `OxCryptobot/un1c0` and **Contents: Read and write**. If the repository belongs to an organization with SAML/SSO, explicitly authorize the token for that organization. Never paste the token into chat, place it in a URL, commit it, or echo it in a shell command.

```bash
env -u GH_TOKEN gh auth login -h github.com -p https
env -u GH_TOKEN gh auth setup-git
env -u GH_TOKEN git push origin main
```

For the existing stored credential, use the interactive scope refresh:

```bash
env -u GH_TOKEN gh auth refresh -h github.com -s repo
env -u GH_TOKEN gh auth setup-git
env -u GH_TOKEN git push origin main
```

When device authorization is requested, complete it in the browser before retrying. Do not start repeated refreshes while an earlier device flow is still pending.

## 4. Branch protection path

After authentication succeeds, inspect branch rules. If `main` disallows direct pushes, publish a feature branch and open a pull request:

```bash
git switch -c chore/publish-local-agent-commits
git push -u origin chore/publish-local-agent-commits
gh pr create --base main --head chore/publish-local-agent-commits \
  --title "Publish local agent-system implementation" \
  --body "Publishes the verified local agent-system and production hardening commits."
```

## References

[1]: https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/creating-a-personal-access-token "GitHub: Managing your personal access tokens"
[2]: https://docs.github.com/en/authentication/connecting-to-github-with-ssh "GitHub: Connecting to GitHub with SSH"
[3]: https://docs.github.com/en/authentication/authenticating-with-saml-single-sign-on "GitHub: Authorizing a personal access token for use with SAML single sign-on"
[4]: https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches "GitHub: Managing protected branches"
