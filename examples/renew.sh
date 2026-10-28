#!/bin/sh
# Rinnova i profili di provisioning dell'iPhone che scadono entro 3 giorni.
# Lo lancia cron (vedi altkeeper.cron). Data ed esito vanno in renew.log.
# Notifiche su ntfy (opzionali, vedi notifica.sh): rinnovi riusciti ed errori
# (al massimo uno ogni 24 ore, perché il telefono può essere fuori casa).
#
# Mettilo nella stessa cartella di `AltKeeper`, `rp-pairing.plist` e `account.json`.

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR" || exit 1

# IP:porta dell'iPhone (di solito la porta è 49152). Vuoto = il telefono si cerca in rete via
# Bonjour, e funziona anche se il suo IP cambia. Se la variabile è già impostata vale quella.
export ALTKEEPER_PHONE="${ALTKEEPER_PHONE:-}"

. ./notifica.sh

OUT=$(./AltKeeper renew --min-days 3 2>&1)
RC=$?
{
  echo "=== $(date "+%F %T")"
  echo "$OUT"
  echo "esito: $RC"
} >> renew.log

if [ "$RC" -ne 0 ]; then
  # Errore: una notifica ogni 24 ore al massimo.
  if [ ! -f .ultimo-errore ] || [ -n "$(find .ultimo-errore -mmin +1440)" ]; then
    notifica "altkeeper: rinnovo non riuscito" "$(echo "$OUT" | tail -n 3 | cut -c1-300)" 4 warning
    touch .ultimo-errore
  fi
else
  rm -f .ultimo-errore
  N=$(echo "$OUT" | grep -c -E "installed on the phone|installato sul telefono")
  if [ "$N" -gt 0 ]; then
    APPS=$(echo "$OUT" | grep -E "downloading a new profile|scarico un profilo nuovo" | sed -E "s/^ +([^:]+):.*/\1/" | sort -u | tr "\n" " ")
    notifica "altkeeper: app rinnovate" "$APPS" 3 white_check_mark
  fi
fi
exit "$RC"
