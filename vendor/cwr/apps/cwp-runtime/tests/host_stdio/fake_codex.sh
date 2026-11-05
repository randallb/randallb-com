#!/bin/sh
if [ "$1 $2" = "login status" ]; then
  exit 0
fi
if [ "$1 $2" = "app-server --stdio" ]; then
  while IFS= read -r request; do
    case "$request" in
      *'"id":"cwr-model-catalog-initialize"'*)
        printf '%s\n' '{"id":"cwr-model-catalog-initialize","result":{}}'
        ;;
      *'"id":"cwr-model-catalog-1"'*)
        printf '%s\n' '{"id":"cwr-model-catalog-1","result":{"data":[],"nextCursor":null}}'
        ;;
    esac
  done
  exit 0
fi
exit 2
