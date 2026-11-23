# sqlpool — operations: SSH, data sources, backup and reset

This page is for setting up and running the parts of sqlpool that touch servers and local Docker:
the SSH login, the server's Docker permissions, data sources, the backup button, and the reset script.

sqlpool itself only reads. The reset script is the one place that drops a database, and it runs in a terminal, not in the app.

## 1. SSH

sqlpool and the scripts reach a server with the system `ssh`. They never ask for a password, so key login must work first.

### 1.1 `~/.ssh/config`

One `Host` block per server. The alias (`quizbuks-staging` here) is what the app and the scripts use.

```
Host quizbuks-staging
    HostName <server address or domain>
    User deploy
    Port 22
    IdentityFile ~/.ssh/id_ed25519
    IdentitiesOnly yes
```

- `User` is the login user on the server. For the staging server it is `deploy`, not `root`.
- `IdentityFile` is the private key whose public half is in the server's `~/.ssh/authorized_keys`.

### 1.2 Check it

```bash
ssh quizbuks-staging 'whoami && docker ps --format "{{.Names}}"'
```

This must print `deploy` and the container names, with no password prompt. If it asks for a password or says `Permission denied (publickey)`, fix the key or `authorized_keys` before going on.

### 1.3 Docker permission on the server

The login user must be able to run `docker` without `sudo`:

```bash
ssh quizbuks-staging 'id'            # must list the group "docker"
```

If it does not, ask the server's admin to run `sudo usermod -aG docker deploy` (the user must log in again afterwards).

### 1.4 Where files go on the server

The login user can write to its own home directory (`~`, here `/home/deploy`). It cannot write to `/root`. Dumps go to `~` by default. Change the folder in the backup tab if needed.

## 2. Local Docker

The local PostgreSQL containers are used for restored copies and for the tests:

| Container | Port | Purpose |
|---|---|---|
| `quizbuks-staging` | 55433 | restored copies of the staging database (the backup tab restores here) |
| `sqlpool-pg` | 55432 | PostgreSQL used by the Rust tests (`SQLPOOL_TEST_PG`) |

The user for both is `postgres`. The password of `quizbuks-staging` is the container's `POSTGRES_PASSWORD`; check it with `docker inspect` if you have forgotten it.

Docker must be running. The app looks for the `docker` binary in `/usr/local/bin`, `/opt/homebrew/bin`, and the Docker.app bundle, because a GUI app does not inherit the shell's `PATH`.

## 3. Data sources

Data sources are managed in the **Data sources** panel (the database icon in the activity bar). Each one is either:

- **Direct**: host, port, database, user and password. The password is stored in the macOS Keychain, never in the database or a file.
- **Over ssh** (tick "Run over ssh"): the query runs inside the container on the server, through `ssh HOST docker exec -i CONTAINER psql ...`. Fill in the SSH host alias (e.g. `quizbuks-staging`), the container name (`postgres`), the database and the user. No password is needed, because the server's own `psql` connects through the local socket.

Test a data source with **Test connection** in its editor. Mark one as the default with **Use as default** (or in the top bar). Statements and environments that do not choose a data source use the default.

Connection strings look like this (they are built by the app; you do not type them):

```
postgres://user:password@host:5432/database      (direct)
ssh://quizbuks-staging/postgres?user=postgres&db=quizbuks_staging   (over ssh)
```

## 4. Backup button

In the **Data sources** panel, the download icon opens **Back up a server database**. One button does:

1. `pg_dump -Fc` on the server, into the dump folder (default `~`), named `DATABASE-DATE.dump`.
2. Download the dump into the app's backups folder (`~/Library/Application Support/com.sqlpool.app/backups`).
3. Copy it into the local container (`quizbuks-staging` by default) and `createdb` a new database named `DATABASE_yyyymmdd`. Existing databases are never touched.
4. `pg_restore` the dump into the new database and create a direct data source for it. The local password you type is saved to the Keychain.

Fields:

| Field | Default |
|---|---|
| Server (ssh) | `quizbuks-staging` |
| Dump folder on server | `~` |
| Server container | `postgres` |
| Server database | `quizbuks_staging` |
| Local container | `quizbuks-staging` |
| Local port | `55433` |
| Local user | `postgres` |
| Local password | (type the container's password) |
| New database | `DATABASE_yyyymmdd` |
| Data source name | `Backup DATABASE yyyymmdd` |

The dump stays on the server and in the backups folder; nothing is deleted.

## 5. Reset a database (`tools/reset_db.sh`)

Resetting drops the database and recreates it, keeping the roles and passwords. It is a script, run in a terminal, so that the steps and the confirmation are explicit.

```bash
tools/reset_db.sh [--dry-run] [--no-restore] SSH_HOST DATABASE [CONTAINER] [REMOTE_DIR]
```

- `SSH_HOST` — the ssh alias, e.g. `quizbuks-staging`
- `DATABASE` — the database to reset, e.g. `quizbuks_staging`
- `CONTAINER` — the postgres container on the server (default `postgres`)
- `REMOTE_DIR` — where the dump is written on the server (default `~`)

Options:

- `--dry-run` prints every command and changes nothing. Run it first.
- `--no-restore` drops and recreates the database but does not restore the dump, leaving it empty.

Environment:

- `BACKUP_DIR` — local folder for the downloaded dumps (default `$HOME/sqlpool-backups`).

What it does, in order:

1. Asks you to type the database name to confirm. Anything else stops with no change.
2. Dumps the database on the server with `pg_dump -Fc`.
3. Downloads the dump to `BACKUP_DIR` and checks it is not empty.
4. `DROP DATABASE … WITH (FORCE)` and `CREATE DATABASE … OWNER`, the owner being the database name, as on the server.
5. Restores the dump into the new database (`pg_restore --no-owner --no-privileges`), unless `--no-restore`.

Any failure stops the script. The dump is kept locally and on the server, so the reset can be undone by restoring it.

The script refuses `postgres`, `template0` and `template1`, and only accepts plain names for the server, container and database.

Example:

```bash
tools/reset_db.sh --dry-run quizbuks-staging quizbuks_staging
tools/reset_db.sh quizbuks-staging quizbuks_staging
```

## 6. Server commands

The **Server commands** panel (the terminal icon in the activity bar) keeps shell commands that run on a server over ssh. It works like Saved SQL:

- Commands are saved in the app's database, filed in folders (drag a command onto a folder, or use the folder's `+`), and can be edited and saved.
- A command can have `:name` parameters, the same as a saved SQL statement. Each value is quoted for the shell (`'…'`, with `'` written as `'\''`), so a value cannot add a command. An empty value is an empty string.
- The **Server (ssh)** field in the command's tab chooses the ssh alias the command runs on.
- **Save and run** saves the command, runs it, and puts the output in the results area below: one row per line, with a `stream` column (stdout or stderr). Export CSV, Keep result, and Runs work on it, so the output can be analysed later.

Output is masked before it is shown: a variable whose name contains `PASS`, `SECRET`, `TOKEN`, `KEY`, `CREDENTIAL` or `PWD` has its value replaced by `********`. A value the command already reduced to a length, such as `(length: 12)`, is kept.

The command text is run by the server's login shell, so write it as a shell script would be written, and avoid anything destructive without a backup first (see section 5).

The first saved command is **Mail settings in the app container**: the `SPRING_MAIL*` and `JABIZ_MAIL*` variables of `quizbuks-staging-app`, with the password shown only as its length.

## 7. Troubleshooting

| Message | Cause | Fix |
|---|---|---|
| `Permission denied (publickey)` | key not accepted | check `IdentityFile` and the server's `authorized_keys` |
| `bash: line 1: /root/….dump: Permission denied` | the login user cannot write to `/root` | use the default dump folder `~` |
| `permission denied while trying to connect to the Docker daemon` | the user is not in the `docker` group | section 1.3 |
| `password authentication failed for user "postgres"` | the data source has the wrong password | edit the data source, type the container's password, save |
| `the database … already exists` | `createdb` found a database with that name | choose another name in the backup tab, or drop the old copy yourself |
| `connect: … Connection refused` on a direct data source | the container is not running or the port is wrong | `docker ps`, check the port mapping |

## 8. Checklist for a new server

1. Add the `Host` block to `~/.ssh/config` (section 1.1) and check the login (1.2).
2. Confirm Docker works for the login user (1.3).
3. Add the data source over ssh, test it, and make it the default (section 3).
4. Run a backup from the backup tab, or `tools/reset_db.sh --dry-run …` first.
5. Check the mail settings from the Server commands panel, if the app sends mail.
6. Add the Anthropic API key in Settings → AI analysis, if you want the AI analysis.

## 9. AI analysis

Any result (a query's rows, or a server command's output in the Console) can be analysed by Claude. Press **Analyze** in the results area.

How it works:

1. The system picks the documents about the result's tables and columns from the Knowledge folder (`repo_quizbuks`, indexed on start; "Index now" in Settings). The total is kept within about 60 KB. You can untick any of them.
2. **Preview what is sent** builds the exact request and shows it: the question, the SQL or command, the description, the first 20 rows, and the documents. Nothing is sent yet.
3. **Send to AI** sends that same text to the Anthropic API. The answer appears in the tab, in four parts: findings, what the documents say, the model's inference (marked), and what it could not tell.

Privacy, before anything is sent:

- Columns whose names say they hold personal data (email, phone, password, token, address, card, and similar) are sent as `[redacted]`.
- E-mail addresses and long digit runs (phone and card numbers) inside any value are replaced with `[redacted]`.
- Values are cut at 300 characters, and only 20 rows are sent.

**Save analysis** keeps the analysis: its name, the request as it was sent (after your edits), the answer and the model. Each analysis belongs to the result it was made from. In the Saved results panel a result is shown with its analyses underneath it. Pressing Analyze on a result that is not kept yet keeps it first. Deleting a kept result also deletes the analyses made from it. Opening an analysis shows the same tab again; delete it from its tab.

The API key is stored in the macOS Keychain, from Settings → AI analysis, and is read only by the backend. Remove it there at any time. The model is chosen in the same section (Claude Sonnet 5.5 by default).

## 10. Generate SQL or a command with AI

Saved SQL and Server command tabs have a **Generate with AI** button. The AI writes the statement from the **Description**, using the notes you add and the documents and tables it finds:

1. The Generate tab (opened from the button) lists the system documents it suggests (you can untick them) and the tables the Description names (SQL only: from the mapping file, with their columns). Only names are sent for the tables, never data.
2. **Preview the request** shows the request; you can edit it. **Send to AI** sends that text.
3. The answer is a *proposal*. It is checked before you see it: SQL must be one SELECT or WITH, with no write words; a command gets warnings for risky words (`rm`, `sudo`, `dd`, `shutdown`, and so on) and for several lines. Nothing is refused for a warning; you decide.
4. **Accept** puts the text in the editor as a change. Nothing is saved or run until you press Save, Save and run, or Run.

Accepting closes the Generate tab and puts the text into the editor it came from. The AI is asked to answer with one code block only.
