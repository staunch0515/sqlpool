#!/usr/bin/env bash
# Reset a database on a server (drop and recreate it, keeping the roles and passwords):
#   1. dump it on the server (pg_dump -Fc, as the server's own backup does)
#   2. download the dump here, and keep it
#   3. drop the database and create it again, with the same owner
#   4. restore the dump into the new, empty database
# Any failure stops the script. The dump is kept, so the reset can be undone.
#
# Usage:
#   tools/reset_db.sh [--dry-run] [--no-restore] SSH_HOST DATABASE [CONTAINER] [REMOTE_DIR]
#     SSH_HOST    the ssh alias of the server, e.g. quizbuks-staging
#     DATABASE    the database to reset, e.g. quizbuks_staging
#     CONTAINER   the postgres container on the server (default: postgres)
#     REMOTE_DIR  where the dump is written on the server (default: ~, the login user's home)
#
# Environment:
#   BACKUP_DIR  local folder for the downloaded dumps (default: $HOME/sqlpool-backups)
set -euo pipefail

DRY_RUN=0
RESTORE=1
ARGS=()
for a in "$@"; do
  case "$a" in
    --dry-run) DRY_RUN=1 ;;
    --no-restore) RESTORE=0 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) ARGS+=("$a") ;;
  esac
done

if [ "${#ARGS[@]}" -lt 2 ]; then
  echo "usage: $0 [--dry-run] [--no-restore] SSH_HOST DATABASE [CONTAINER] [REMOTE_DIR]" >&2
  exit 2
fi
HOST="${ARGS[0]}"
DB="${ARGS[1]}"
CONT="${ARGS[2]:-postgres}"
RDIR="${ARGS[3]:-~}"
BACKUP_DIR="${BACKUP_DIR:-$HOME/sqlpool-backups}"

# Names go into shell commands on the server: only plain names are allowed.
name_ok() { [[ "$1" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,62}$ ]]; }
for v in "$HOST" "$CONT" "$DB"; do
  name_ok "$v" || { echo "not allowed: $v" >&2; exit 2; }
done
case "$DB" in postgres|template0|template1) echo "refusing to reset $DB" >&2; exit 2 ;; esac
[[ "$RDIR" == "~" || "$RDIR" == /* || "$RDIR" == "~/"* ]] || { echo "the server folder must be ~ or an absolute path" >&2; exit 2; }

DATE="$(date +%F)"
STAMP="$(date +%Y%m%d-%H%M%S)"
REMOTE_FILE="$RDIR/$DB-$DATE.dump"
LOCAL_FILE="$BACKUP_DIR/$HOST-$DB-$STAMP.dump"

run() {
  if [ "$DRY_RUN" = 1 ]; then echo "DRY RUN: $*"; else "$@"; fi
}

echo "Server:    $HOST (container $CONT)"
echo "Database:  $DB"
echo "Dump:      $REMOTE_FILE on the server, kept as $LOCAL_FILE here"
echo "Steps:     dump, download, drop and create ($DB owns it), $([ "$RESTORE" = 1 ] && echo restore || echo 'no restore')"
echo

# The reset throws the data away, so the database name must be typed in.
if [ "$DRY_RUN" = 0 ]; then
  if [ ! -t 0 ] && [ ! -r /dev/tty ]; then echo "run this in a terminal" >&2; exit 2; fi
  read -r -p "Type the database name ($DB) to confirm the reset: " typed < /dev/tty
  [ "$typed" = "$DB" ] || { echo "not confirmed; nothing was changed" >&2; exit 1; }
  mkdir -p "$BACKUP_DIR"
fi

echo "1/4 dump on the server"
run ssh -o BatchMode=yes "$HOST" "docker exec $CONT pg_dump -U postgres -Fc $DB > $REMOTE_FILE"

echo "2/4 download the dump"
if [ "$DRY_RUN" = 1 ]; then
  echo "DRY RUN: ssh $HOST 'cat $REMOTE_FILE' > $LOCAL_FILE"
else
  ssh -o BatchMode=yes "$HOST" "cat $REMOTE_FILE" > "$LOCAL_FILE"
  [ -s "$LOCAL_FILE" ] || { echo "the downloaded dump is empty; stopping" >&2; exit 1; }
  echo "    saved $(wc -c < "$LOCAL_FILE" | tr -d ' ') bytes"
fi

echo "3/4 drop and create the database"
run ssh -o BatchMode=yes "$HOST" "docker exec $CONT psql -U postgres -v ON_ERROR_STOP=1 -c 'DROP DATABASE \"$DB\" WITH (FORCE);' -c 'CREATE DATABASE \"$DB\" OWNER \"$DB\";'"

if [ "$RESTORE" = 1 ]; then
  echo "4/4 restore the dump into the new database"
  if [ "$DRY_RUN" = 1 ]; then
    echo "DRY RUN: ssh $HOST 'docker exec -i $CONT pg_restore -U postgres -d $DB --no-owner --no-privileges' < $LOCAL_FILE"
  else
    ssh -o BatchMode=yes "$HOST" "docker exec -i $CONT pg_restore -U postgres -d $DB --no-owner --no-privileges" < "$LOCAL_FILE"
  fi
else
  echo "4/4 restore skipped (--no-restore)"
fi

echo
echo "Done. The dump is kept at $LOCAL_FILE"
