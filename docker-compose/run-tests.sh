#!/bin/bash
set -x

extdir=${1}

cd "${extdir}" || exit 2
FAILED=
LIST=$( (echo -e "${SKIP//","/"\n"}"; ls) | sort | uniq -u)
for d in ${LIST}
do
       [ -d "${d}" ] || continue
    psql -c "select 1" >/dev/null || break
       make -C "${d}" installcheck || FAILED="${d} ${FAILED}"
done
[ -z "${FAILED}" ] && exit 0
echo "${FAILED}"
exit 1