# Sign-in with your identity provider

People can sign in to OpSkills with your company's identity provider, over OpenID
Connect (OIDC): Okta, Microsoft Entra ID, Google Workspace, Keycloak, Authentik and
others. One provider at a time, set up by a server admin under **Settings > Sign-in**.

Passwords keep working. The first admin still signs in with the generated password from
the first start and chooses their own ([Install](install.md)); a provider is something
you add afterwards. You can then limit passwords to server admins, so they keep a way in
if the provider is down.

OpSkills talks to the provider itself. It never trusts identity headers from a proxy,
so nobody can sign in by sending made-up headers to the server.

## Before you start

- Set the server's public URL (`server.public_url`, or `OPSKILLS_SERVER__PUBLIC_URL`),
  for example `https://ops.example.com`. The provider sends people back to it.
- The redirect URL to register at the provider is the public URL plus
  `/api/v1/auth/oidc/callback`, for example
  `https://ops.example.com/api/v1/auth/oidc/callback`. **Settings > Sign-in** shows it
  with a copy button.

## Set up the provider

1. At the provider, create a **web application** (OpenID Connect, authorization code
   flow) with the redirect URL above, and the scopes `openid`, `email` and `profile`.
   Note its client ID and client secret.
2. In OpSkills, add the client secret as a shared secret under **Vault > Shared**, for
   example `oidc/CLIENT_SECRET`. It doesn't need to be shared with any workspace.
3. Under **Settings > Sign-in**, fill in:
   - **Button label**, shown as "Sign in with <label>";
   - **Issuer URL**: press **Check** to confirm OpSkills can read the provider's
     settings;
   - **Client ID** and the **client secret** you added.
4. Choose the account options (below) and **Save**.

| Provider | Issuer URL | Notes |
|---|---|---|
| Okta | `https://<your-okta-domain>`, the org authorization server | See [Okta, step by step](#okta-step-by-step). A custom authorization server (`.../oauth2/default`) also works, but needs Okta API Access Management outside free developer orgs |
| Microsoft Entra ID | `https://login.microsoftonline.com/<tenant-id>/v2.0` | Add the optional `email` claim to the ID token. Entra ID doesn't send `email_verified`; turn on **Trust emails the provider doesn't mark verified** if you link or create accounts by email |
| Google | `https://accounts.google.com` | With email domains set, people must also be in one of those Google Workspace domains, not only use an address there |
| Keycloak | `https://<host>/realms/<realm>` | |

## Okta, step by step

This uses Okta's org authorization server, which is meant for signing in to your own
apps and needs no add-on. Replace `ops.example.com` with your OpSkills address and
`example.okta.com` with your Okta domain.

**In the Okta Admin Console**

1. Go to **Applications > Applications** and choose **Create App Integration**.
2. Pick **OIDC - OpenID Connect** as the sign-in method and **Web Application** as the
   application type, then **Next**.
3. Fill in:
   - **App integration name**: `OpSkills`;
   - **Grant type**: **Authorization Code** only;
   - **Sign-in redirect URIs**: `https://ops.example.com/api/v1/auth/oidc/callback`
     (Settings > Sign-in in OpSkills shows yours);
   - **Sign-out redirect URIs**: leave empty.
4. Under **Assignments**, choose who may sign in: everyone in your organization, or
   **Limit access to selected groups** (for example a group `OpSkills users`). Okta only
   lets assigned people sign in to the app. **Save**.
5. On the app's **General** tab, under **Client Credentials**: keep **Client
   authentication** on **Client secret**, and turn on the **Proof Key for Code Exchange
   (PKCE)** requirement (OpSkills always sends one). Copy the **Client ID** and the
   **Client secret**.

**In OpSkills, as a server admin**

6. Open **Vault > Shared**, choose **Add shared secret**, and save the client secret
   with the folder `oidc` and the name `OKTA_CLIENT_SECRET`. Don't share it with any
   workspace: only the server uses it.
7. Open **Settings > Sign-in** and fill in:
   - **Button label**: `Okta`;
   - **Issuer URL**: `https://example.okta.com`, then press **Check**: it should find
     the sign-in page and at least one signing key;
   - **Client ID**: from step 5;
   - **Client secret**: `shared/oidc/OKTA_CLIENT_SECRET`.
8. Choose how accounts are matched (see [Who can sign in](#who-can-sign-in)):
   - to let existing users in, give each one their Okta email under **Settings > Users >
     Email**; **Link accounts by email** is on by default;
   - to let new people in, turn on **Create accounts on first sign-in** and list your
     email domain, like `example.com`. They start in no workspace.

   **Save**.
9. In a private window, open OpSkills and choose **Sign in with Okta**. After Okta, you
   land in OpSkills as the linked user. If the page says Okta hasn't verified the email,
   turn on **Trust emails the provider doesn't mark verified** (Okta's primary emails
   are managed by your Okta admins) and try again.
10. Once everyone signs in with Okta, you can turn on **Passwords only for server
    admins**.

Okta groups don't become OpSkills roles: give roles in OpSkills, under each workspace's
**Members**.

## Who can sign in

An account is linked to the provider by the provider's own id for the person, so a
changed or reused email address can't take it over. The first time someone signs in
with the provider:

1. **An account with their email.** Give people an email under **Settings > Users**
   (when you add them, or with **Email** on the list). With **Link accounts by email**
   on (the default), their first sign-in links to that account when the provider says
   the email is verified. A generated password they never replaced stops working then.
2. **A new account.** With **Create accounts on first sign-in** on, people from the
   **email domains** you list get an account. It's in no workspace and not a server
   admin, so they see nothing until a workspace Admin adds them
   ([Workspaces](workspaces.md#members)). The user name comes from the provider's user
   name or the email, with a number added if it's taken. Off by default.
3. **Anyone else** is sent back to the sign-in page with the reason, like "there's no
   OpSkills account for jane@example.com. Ask a server admin to add you."

Roles stay in OpSkills: server admins and workspace Admins give them, as for any user.
Groups at the provider don't become OpSkills roles.

A server admin can **Unlink** a user from their provider account; their next sign-in
links again by email. Removing a user ends their sessions at once. Disabling someone
only at the provider ends their access when their OpSkills session expires
(`auth.session_hours`, 12 hours by default), so remove them in OpSkills too.

## Passwords only for server admins

With a provider on, **Passwords only for server admins** makes everyone else use the
provider: a password sign-in by anyone but a server admin is refused with "Passwords
are only for server admins here. Sign in with <label>." Accounts created by the
provider have no password; a server admin can still give one with **Reset password**.

## Signing out

**Sign out** ends the OpSkills session. Your session at the provider stays, so signing
in again may not ask for your password there.

## If it doesn't work

The sign-in page shows why a provider sign-in failed, and the audit log records it
(`auth.sign_in_failed`, "with the sign-in provider: ...").

| Message | What to check |
|---|---|
| Couldn't reach the sign-in provider | The issuer URL (use **Check**), and that the server can reach it |
| The provider refused the sign-in code | The client secret, and the redirect URL registered at the provider |
| The ID token didn't check out | The client ID, the server's clock, and that the issuer is exactly the provider's |
| This sign-in has expired or was started in another browser | Start again from the sign-in page, in the same browser, within 10 minutes |
| The provider hasn't verified ... / sent no email address | The provider's email claim, or **Trust emails the provider doesn't mark verified** |

## API

| Endpoint | Who | What it does |
|---|---|---|
| `GET /settings/sign-in` | Server admins | The settings and the redirect URL to register |
| `PUT /settings/sign-in` | Server admins | `{"oidc": {"label", "issuer", "client_id", "client_secret": "shared/...", "enabled", "link_by_email", "create_accounts", "allowed_domains": [...], "trust_unverified_email"}, "passwords_for_server_admins_only": false}`; `"oidc": null` removes the provider |
| `POST /settings/sign-in/test` | Server admins | `{"issuer"}`: reads the provider's settings |
| `GET /auth/oidc/start` | Anyone | Goes to the provider (the sign-in button) |
| `GET /auth/oidc/callback` | The provider | Where the provider sends people back |

`GET /system` tells the sign-in page what to show: `"sign_in": {"provider": "<label>"
or null, "passwords": "everyone" or "server_admins"}`.
