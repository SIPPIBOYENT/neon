#!/bin/sh
set -ex
cd "$(dirname ${0})"
PG_REGRESS=$(dirname "$(pg_config --pgxs)")/../test/regress/pg_regress
${PG_REGRESS} --inputdir=./ --bindir='/usr/local/pgsql/bin' --use-existing --encoding=UTF8 --dbname=contrib_regression init base_plan pg_hint_plan ut-init ut-S ut-J ut-L ut-G ut-R ut-fdw ut-W ut-T ut-fini hints_anywhere plpgsql