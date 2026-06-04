#!/usr/bin/env bash
# Download the full RSQA archive into data-src/ (git-ignored).
#
#   - 39 annual multi-pollutant CSVs (1986–2024) → rsqa-multi-polluants<YEAR>.csv
#   - 6 IQA "détaillé par station" bundles (2007–2024) → rsqa-indice-qualite-air-<range>.csv
#   - the station list
#
# Files already present are skipped, so re-runs only fetch what's missing.
# A polite delay between requests respects the portal's crawl etiquette
# (robots.txt allows these resource download/ URLs; only /api/ is disallowed).
set -euo pipefail

DEST="$(dirname "$0")/../data-src"
mkdir -p "$DEST"
DELAY="${RSQA_FETCH_DELAY:-3}"
GAZ="https://donnees.montreal.ca/dataset/33a1a2d5-cebe-41d3-b98d-f64de6df4d5e/resource"
IQA="https://donnees.montreal.ca/dataset/547b8052-1710-4d69-8760-beaa3aa35ec6/resource"

# year:resource-id for the multi-pollutant annual files
GAZ_FILES=(
  "1986:618d974a-f443-4502-aed3-9e350afee59a" "1987:bac166b6-662b-4dac-ac0e-190c256e7f9c"
  "1988:014ed0d4-accd-4c26-b76d-05492386edbc" "1989:803204c3-ccc0-4d92-8e0f-8d6e7e1c804e"
  "1990:450d8754-f683-4575-b8af-aea40f29553d" "1991:2ff2b1b0-037e-4cbb-99eb-b7f76fa8638e"
  "1992:983e655d-dd61-4469-bfe9-47f07ff5b356" "1993:cb9f8b90-b8b9-44b8-ae6c-d6d4efa7d011"
  "1994:4325c372-84e4-46a9-9460-506c43162abb" "1995:d25a6417-d822-4168-b7f9-e3f1a5c649a4"
  "1996:b87fc94b-47b0-47b2-9c5b-0c03df3ada72" "1997:34eb2198-58c1-4c0f-958c-1bdf706144f8"
  "1998:8280088c-de58-4e4c-8d02-edd680161bfa" "1999:260942b5-934d-419d-ad9f-9f27c7bf2ffc"
  "2000:8966902b-9bab-4d58-8e2b-1a3b8703035c" "2001:9574afb4-1527-4218-956a-d968e7458785"
  "2002:32c5a36b-ca2e-43c4-adc9-2609139ee53d" "2003:43e1003f-059b-4bf1-968d-8c29d464fbba"
  "2004:e921bb41-8a52-4fd9-a547-d0b07ce311c8" "2005:69fe9f32-0714-4236-a4e0-488974017de8"
  "2006:1be53c18-7218-4d3b-82b9-72c7f21c60a8" "2007:805b4772-9c47-4ff2-a6f7-5c6a70ce9869"
  "2008:28aaa3f2-3cc2-482c-8a2d-3b9d19d2ef58" "2009:48599928-4d7c-4122-9bfa-43624d364875"
  "2010:ae3be448-6b04-45ca-845c-1db3828ce18b" "2011:b93fb09e-08ff-4921-af0b-896ebdb96edd"
  "2012:e703f3a7-edbe-47a7-a958-65e9f9644eaf" "2013:033c2090-0c4a-40ec-97e9-85269465daa3"
  "2014:39c2ed39-577c-48d8-9a7b-9063fa2ae647" "2015:fb449c46-ac3e-46a6-82f0-f98a24384d67"
  "2016:5bcd5f89-e23d-4a4d-8e59-237b6c2979f7" "2017:c17bf9d6-f113-4ce3-a4b1-4661841f78be"
  "2018:1a4616a2-d955-4344-8ec8-da6cb8d05864" "2019:d2bf031a-394f-4b75-ba26-fab00c895209"
  "2020:41ca8161-a325-4a76-9007-3643054e07cb" "2021:4b5b2293-c834-4228-95eb-6311720af143"
  "2022:dd5f3a81-5321-48db-a2b1-75ab6d42dccf" "2023:89e6aded-81a1-4514-8658-0457e4c43aab"
  "2024:307e46d9-fbdf-48ad-8acd-f38774cd80c2"
)

# range:resource-id for the IQA detailed-by-station bundles
IQA_FILES=(
  "2007-2009:a4a7cc31-6a55-40c3-852b-08ade7e91f8e" "2010-2012:6e59c6e9-749b-4237-9c45-5c627be2b7ad"
  "2013-2015:02cfaf0c-3b46-4dac-bf66-acd2ff47361a" "2016-2018:93a3a88e-97ab-4ab1-813f-5419a1dd330d"
  "2019-2021:e43dc1d6-fbdd-49c3-a79f-83f63404c281" "2022-2024:0c325562-e742-4e8e-8c36-971f3c9e58cd"
)

fetch() { # url, outfile
  if [ -s "$DEST/$2" ]; then echo "skip  $2"; return; fi
  echo "get   $2"
  curl -fsSL -A "Mozilla/5.0" "$1" -o "$DEST/$2"
  sleep "$DELAY"
}

for e in "${GAZ_FILES[@]}"; do
  y="${e%%:*}"; rid="${e##*:}"
  fetch "$GAZ/$rid/download/rsqa-multi-polluants$y.csv" "rsqa-multi-polluants$y.csv"
done
for e in "${IQA_FILES[@]}"; do
  r="${e%%:*}"; rid="${e##*:}"
  fetch "$IQA/$rid/download/rsqa-indice-qualite-air-$r.csv" "rsqa-indice-qualite-air-$r.csv"
done

echo "Archive ready in $DEST"
