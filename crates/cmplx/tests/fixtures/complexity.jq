# Verbatim copy of the `jq -rs --argjson max "$MAX"` program in
# ~/code/skills/hooks/complexity.sh (skills repo at 7369624; last changed by 3d1e64b).
# Run as: jq -rs --argjson max N -f complexity.jq  on `[after, before]`.
  ((.[1].functions // []) | group_by(.name)
    | map({key: (.[0].name // "?"), value: (map(.complexity) | max)}) | from_entries) as $was
  | (.[0].functions // [])[]
  | (.name // "?") as $n
  | select(.complexity > $max and (($was[$n] // -1) < .complexity))
  | [.line, $n, .complexity, ($was[$n] // "new")] | @tsv
