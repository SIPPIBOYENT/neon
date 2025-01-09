#!/bin/bash
set -eux -o pipefail
# Takes a variable name as argument. The result is stored in that variable.
generate_id() {
    local -n resvar=$1
    printf -v resvar '%08x%08x%08x%08x' $SRANDOM $SRANDOM $SRANDOM $SRANDOM
}
TENANT_ID_FILE="$(dirname ${0})/compute_wrapper/var/db/postgres/specs/tenant_id"
TIMELINE_ID_FILE="$(dirname ${0})/compute_wrapper/var/db/postgres/specs/timeline_id"
rm -f "${TENANT_ID_FILE}" "${TIMELINE_ID_FILE}"
if [ -z ${OLDTAG+x} ] || [ -z ${NEWTAG+x} ] || [ -z "${OLDTAG}" ] || [ -z "${NEWTAG}" ]; then
  echo OLDTAG and NEWTAG must be defined
  exit 1
fi
export PG_VERSION=${PG_VERSION:-16}
export PGUSER=cloud_admin
export PGPASSWORD=cloud_admin
export PGHOST=127.0.0.1
export PGPORT=55433
export PGDATABASE=postgres
docker volume prune -f
function wait_for_ready {
  while ! docker compose logs compute_is_ready | grep -q "accepting connections"; do
    sleep 1
  done
}
function create_extensions() {
  for ext in ${1}; do
    echo "CREATE EXTENSION IF NOT EXISTS ${ext};"
  done | psql -X -v ON_ERROR_STOP=1
}
EXTENSIONS='[
{"extname": "plv8", "extdir": "plv8-src"},
{"extname": "vector", "extdir": "pgvector-src"},
{"extname": "unit", "extdir": "postgresql-unit-src"},
{"extname": "hypopg", "extdir": "hypopg-src"},
{"extname": "rum", "extdir": "rum-src"},
{"extname": "ip4r", "extdir": "ip4r-src"},
{"extname": "prefix", "extdir": "prefix-src"},
{"extname": "hll", "extdir": "hll-src"},
{"extname": "pg_cron", "extdir": "pg_cron-src"}
]'
EXTNAMES=$(echo ${EXTENSIONS} | jq -r '.[].extname' | paste -sd ' ' -)
TAG=${NEWTAG} docker compose up --build -d
wait_for_ready
create_extensions "${EXTNAMES}"
query="select json_object_agg(extname,extversion) from pg_extension where extname in ('${EXTNAMES// /\',\'}')"
new_vers=$(psql -Aqt -c "$query" )
echo $new_vers
docker compose down
TAG=${OLDTAG} docker compose --profile test-extensions up --build -d --force-recreate
wait_for_ready
# XXX this is about to be included into the image, for test only
docker compose cp  ext-src neon-test-extensions:/
psql -c "CREATE DATABASE contrib_regression"
export PGDATABASE=contrib_regression
create_extensions "${EXTNAMES}"
query="select pge.extname from pg_extension pge join (select key as extname, value as extversion from json_each_text('${new_vers}')) x on pge.extname=x.extname and pge.extversion <> x.extversion"
exts=$(psql -Aqt -c "$query")
if [ -z "${exts}" ]; then
  echo "No extensions were upgraded"
else
  tenant_id=$(psql -Aqt -c "SHOW neon.tenant_id")
  timeline_id=$(psql -Aqt -c "SHOW neon.timeline_id")
  #XXX for ext in ${exts}; do
  for ext in ${EXTNAMES}; do
    echo Testing ${ext}...
    EXTDIR=$(echo ${EXTENSIONS} | jq -r '.[] | select(.extname=="'${ext}'") | .extdir')
    generate_id new_timeline_id
    PARAMS=(
        -sbf
        -X POST
        -H "Content-Type: application/json"
        -d "{\"new_timeline_id\": \"${new_timeline_id}\", \"pg_version\": ${PG_VERSION}, \"ancestor_timeline_id\": \"${timeline_id}\"}"
        "http://127.0.0.1:9898/v1/tenant/${tenant_id}/timeline/"
    )
    result=$(curl "${PARAMS[@]}")
    echo $result | jq .
    TENANT_ID=${tenant_id} TIMELINE_ID=${new_timeline_id} TAG=${OLDTAG} docker compose down compute compute_is_ready
    COMPUTE_TAG=${NEWTAG} TAG=${OLDTAG} TENANT_ID=${tenant_id} TIMELINE_ID=${new_timeline_id} docker compose up -d --build compute compute_is_ready
    wait_for_ready
    TID=$(psql -Aqt -c "SHOW neon.timeline_id")
    if [ ${TID} != ${new_timeline_id} ]; then
      echo Timeline mismatch
      exit 1
    fi
    if [ ${ext} == "pg_hintplan" ]; then
        TMPDIR=$(mktemp -d)
        # The following block does the same for the pg_hintplan test
        docker compose cp neon-test-extensions:/ext-src/pg_hint_plan-src/data $TMPDIR/data
        docker compose cp $TMPDIR/data compute:/ext-src/pg_hint_plan-src/
        rm -rf $TMPDIR
    fi
    psql -d contrib_regression -c "\dx ${ext}"
    docker compose exec -e PGPASSWORD=cloud_admin neon-test-extensions sh -c /ext-src/${EXTDIR}/test-upgrade.sh
    psql -d contrib_regression -c "alter extension ${ext} update"
    psql -d contrib_regression -c "\dx ${ext}"
  done
fi
docker compose --profile test-extensions down
rm -f "${TENANT_ID_FILE}" "${TIMELINE_ID_FILE}"
