# Signing in to Microsoft 365 and Gmail

Microsoft 365 and Gmail do not take a password from a mail program. You sign in to the provider in your browser, and the provider hands Quill a sign-in it can use from then on. Quill never sees your password, and keeps the sign-in in the macOS Keychain.

For this the provider has to know which application is asking. That is Quill itself, with both: it is registered with Microsoft and with Google, and you only sign in. Registering an application of your own is for when you would sooner sign in as that, and for Google it is also what a Quill you built yourself from the source needs: Quill's registration with Google is carried by the builds Quill is given out as, and is not in the source.

Nothing here has been checked against the real services yet. The steps follow the providers' documentation as it stood when this was written; if a screen has moved, the names of the settings are what to look for.

## Microsoft 365 and Outlook.com

Quill reads and sends this mail through Microsoft Graph.

To add a Microsoft account, press `A` in Quill, type the address and continue: Microsoft's sign-in page opens in your browser. The rest of this section is only for someone who wants Quill to sign in as an application of their own, such as an organisation that allows only applications it has registered.

### Register an application of your own

1. Go to <https://entra.microsoft.com> and sign in. A work account may need its administrator's leave to register applications; a personal Microsoft account can use <https://portal.azure.com> the same way.
2. In the menu on the left open **Entra ID**, then **App registrations**, then press **New registration**. (Older screens have it under **Applications**. If there is no App registrations at all, or you are told you have no directory, see the note below the list.)
3. Give it a name, such as "Quill".
4. Under **Supported account types** choose "Any Entra ID Tenant + Personal Microsoft accounts" (older screens: "Accounts in any organizational directory and personal Microsoft accounts"), unless you only ever want accounts of your own organisation. An Outlook.com or Hotmail address is a personal account, and needs this choice.
5. Under **Redirect URI** choose the platform **Public client/native (mobile & desktop)** and enter `http://localhost`. Just that: no port and no path.
6. Press **Register**. The **Overview** page that follows shows the **Application (client) ID**, a long code of letters, digits and dashes. That is the application id Quill asks for, and all it needs from this page.
7. Open **Authentication**. Under **Advanced settings** turn on **Allow public client flows**. Signing in with a code from another computer needs it.
8. Open **API permissions**, then **Add a permission**, **Microsoft Graph**, **Delegated permissions**, and add `Mail.ReadWrite`, `Mail.Send`, `User.Read` and `offline_access`.

An application is registered in a directory (a "tenant"). A work or school account has one. A personal Microsoft account may have none, and then the admin center has nowhere to put a registration: Microsoft's own answer is to create a directory first, which a free Azure account does. This guide has not been through that case.

Do not create a client secret. A desktop application cannot keep one, and Microsoft does not ask a public client for one.

### Use it for an account

An application of your own is named in the config, since the form adds Microsoft accounts as Quill itself. Write the account as below. If the config already has a Microsoft account that names an application, the form offers that id for the next one, with a tenant to give.

### The account in config.toml

```toml
[[account]]
name = "Work"
type = "graph"
address = "you@example.com"
oauth_client_id = "the Application (client) ID"
```

If you chose a single organisation in step 4, name it too, by its id or one of its domain names:

```toml
oauth_tenant = "example.com"
```

### Sign in

An account added in Quill goes to this by itself. For one written by hand, start Quill. The account is listed, with a note that nobody has signed in to it yet. Press `S` (or choose "Sign in to an account" from the palette or the File menu). Your browser opens at Microsoft's sign-in page; sign in there and agree to what Quill asks for, and come back to Quill. The mail starts to arrive.

From a terminal the same thing is `quill account sign-in Work`.

Over SSH there is no browser to open on the Mac you are sitting at. Quill shows a short code and an address instead: open the address on any device, type the code in, and sign in there. `quill account sign-in Work --code` asks for this way at the Mac itself.

## Gmail

Quill reads Gmail over IMAP and sends over SMTP, signing in to both with the same sign-in.

To add a Gmail account, press `A` in Quill, type the address and continue: the form offers it as **Google (sign in)**, with nothing to fill in but its name. Continue, and Google's sign-in page opens in your browser; sign in there, agree to what Quill asks for, and come back. The same goes for an address of your own whose mail Google Workspace hosts.

Two things Google may show you on the way, both because Quill's registration with Google has not been through Google's verification:

- **"Google hasn't verified this app."** Choose **Advanced**, then **Go to Quill**.
- **"Access blocked"**, or an error saying the app is being tested. Quill's registration then lets in only the addresses listed for it. Use an app password, or an application of your own, below.

An app password is the other way to use Gmail, and needs none of this: turn on 2-Step Verification, create an app password at <https://myaccount.google.com/apppasswords>, and add the account in Quill with the type **IMAP (password)** and that as its password.

### Register an application of your own

Only for someone who wants Quill to sign in as an application of their own.

1. Go to <https://console.cloud.google.com> and create a project. Any name will do.
2. Under **APIs & Services**, **OAuth consent screen** (it may open as **Google Auth Platform**), set the application up as **External** and give it a name and your address.
3. Under **Audience**, add your own Gmail address to **Test users**. While the application is in **Testing**, only test users can sign in.
4. Under **Data access**, add the scope `https://mail.google.com/`.
5. Under **Clients**, create a client of the type **Desktop app**.
6. The page that follows shows a **Client ID** and a **Client secret**. Both go in the config, below.

Two things to know. Google calls the second value a secret, and says itself that for a desktop application it is not one: every copy of the application carries it. That is why it may sit in `config.toml`, where real passwords never do. And while the application is in Testing, Google ends each sign-in after seven days, so Quill will ask you to sign in again every week. Publishing the application ends that, and for the mail scope Google wants to verify it first.

In a Quill you built from the source, the form asks for these two when you choose the type **Google (sign in)**, and offers them again for the next Google account. In a build that carries Quill's own registration the form adds Google accounts as Quill itself, and an application of your own is named in the config, below.

### The account in config.toml

What the form writes, less the two `oauth_client` lines, which are only for an application of your own:

```toml
[[account]]
name = "Personal"
type = "imap"
address = "you@gmail.com"
auth = "oauth"
oauth_client_id = "the Client ID"
oauth_client_secret = "the Client secret"
imap = { host = "imap.gmail.com", port = 993, tls = "implicit" }
smtp = { host = "smtp.gmail.com", port = 465, tls = "implicit" }
```

### Sign in

As for Microsoft: `S` in Quill, or `quill account sign-in Personal`. Google has no sign-in by code for mail, so this has to be done at the Mac itself.

## Looking after a sign-in

- `quill account list` says for each account whether it is signed in.
- `quill account forget NAME` removes the sign-in from the Keychain. Quill then asks for a new one.
- The sign-in is kept under your address, not the account's name, so renaming an account in the config keeps it.
- Removing Quill's access at the provider (Microsoft: <https://myaccount.microsoft.com>, under privacy and applications; Google: <https://myaccount.google.com/permissions>) ends the sign-in everywhere. Quill notices at its next sync and asks you to sign in again.

## A national cloud

Microsoft's national clouds have their own addresses. Give them in the account:

```toml
[account.oauth]
authorize_url = "https://login.microsoftonline.us/common/oauth2/v2.0/authorize"
token_url = "https://login.microsoftonline.us/common/oauth2/v2.0/token"
device_url = "https://login.microsoftonline.us/common/oauth2/v2.0/devicecode"
graph_url = "https://graph.microsoft.us/v1.0"
```

Each must be `https`.
