#!/usr/bin/env bash
# One-time setup for signing Redlib in as your own Reddit "script" app.
#
# Opens Reddit's approval page for your app; after you click "Allow", paste
# the address your browser lands on (it won't load; that's expected) and this
# prints a long-lived refresh token for REDLIB_REDDIT_REFRESH_TOKEN.
#
# The app's redirect uri at reddit.com/prefs/apps must be exactly:
#   http://localhost:8080
#
# Nothing is written to disk; the secret is read without echoing it.
set -euo pipefail

REDIRECT_URI="http://localhost:8080"
# read: listings, threads, search; history: user profile pages; wikiread: wikis.
SCOPES="read history wikiread"

read -r -p "Client ID (shown under the app name): " CLIENT_ID
read -r -s -p "Secret (input hidden): " CLIENT_SECRET
echo
read -r -p "Your Reddit username (without u/): " USERNAME
[ -n "$CLIENT_ID" ] && [ -n "$CLIENT_SECRET" ] && [ -n "$USERNAME" ] || { echo "All three are required." >&2; exit 1; }

STATE="$(LC_ALL=C tr -dc 'a-zA-Z0-9' </dev/urandom | head -c 24)"
ENC_REDIRECT="$(python3 -I -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$REDIRECT_URI")"
ENC_SCOPES="$(python3 -I -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "$SCOPES")"
AUTH_URL="https://www.reddit.com/api/v1/authorize?client_id=${CLIENT_ID}&response_type=code&state=${STATE}&redirect_uri=${ENC_REDIRECT}&duration=permanent&scope=${ENC_SCOPES}"

echo
echo "Opening Reddit's approval page. Check it names your app, then click \"Allow\"."
echo "If it doesn't open, visit:"
echo "  $AUTH_URL"
open "$AUTH_URL" 2>/dev/null || true
echo
read -r -p "Paste the full address your browser ended up on (starts with ${REDIRECT_URI}): " RETURNED

CODE="$(python3 -I - "$RETURNED" "$STATE" <<'PY'
import sys, urllib.parse
url, expected_state = sys.argv[1], sys.argv[2]
query = urllib.parse.parse_qs(urllib.parse.urlparse(url.strip()).query)
if "error" in query:
    sys.exit(f"Reddit returned an error: {query['error'][0]}")
if query.get("state", [""])[0] != expected_state:
    sys.exit("The state in that address doesn't match this run; start again.")
code = query.get("code", [""])[0].split("#")[0]
if not code:
    sys.exit("No code found in that address.")
print(code)
PY
)"

RESPONSE="$(curl -sS https://www.reddit.com/api/v1/access_token \
	-u "${CLIENT_ID}:${CLIENT_SECRET}" \
	-A "linux:redlib-personal:setup (by /u/${USERNAME})" \
	--data-urlencode "grant_type=authorization_code" \
	--data-urlencode "code=${CODE}" \
	--data-urlencode "redirect_uri=${REDIRECT_URI}")"

python3 -I - "$RESPONSE" <<'PY'
import json, sys
try:
    data = json.loads(sys.argv[1])
except ValueError:
    sys.exit("Reddit returned something other than JSON; try again in a minute.")
if "refresh_token" not in data:
    sys.exit(f"Reddit did not return a refresh token: {data.get('error', data)}")
print()
print("Success. Set these four in the Portainer stack's Environment variables:")
print("  REDLIB_REDDIT_CLIENT_ID      = the client ID you entered")
print("  REDLIB_REDDIT_CLIENT_SECRET  = the secret you entered")
print("  REDLIB_REDDIT_USERNAME       = your username")
print(f"  REDLIB_REDDIT_REFRESH_TOKEN  = {data['refresh_token']}")
print()
print("Treat the refresh token like a password. Revoke it any time at")
print("reddit.com/prefs/apps (under \"authorized applications\").")
PY
