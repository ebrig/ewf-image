#!/usr/bin/env bash
# Refuse release publication unless the tag, package, main, and required checks agree.
set -euo pipefail

tag=${1:-}
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
    echo "Expected an ewf-image release tag such as v0.7.0" >&2
    exit 2
fi

for command in cargo gh git jq; do
    command -v "$command" >/dev/null || {
        echo "Required command is unavailable: $command" >&2
        exit 2
    }
done

tag_ref="refs/tags/$tag"
git rev-parse --verify --quiet "$tag_ref" >/dev/null || {
    echo "Release tag does not exist locally: $tag" >&2
    exit 1
}
if [[ $(git cat-file -t "$tag_ref") != tag ]]; then
    echo "Release tag must be annotated: $tag" >&2
    exit 1
fi

release_commit=$(git rev-parse "${tag_ref}^{}")
if [[ $(git rev-parse HEAD) != "$release_commit" ]]; then
    echo "The checkout is not at the tagged release commit $release_commit" >&2
    exit 1
fi

package_version=$(cargo metadata --no-deps --format-version 1 | jq -r '
    [.packages[] | select(.name == "ewf-image") | .version] as $versions
    | if ($versions | length) == 1 then $versions[0] else empty end
')
if [[ -z "$package_version" || "$tag" != "v$package_version" ]]; then
    echo "Tag $tag does not match ewf-image package version ${package_version:-<missing>}" >&2
    exit 1
fi

git fetch --quiet --no-tags origin main
main_commit=$(git rev-parse refs/remotes/origin/main)
if [[ "$release_commit" != "$main_commit" ]]; then
    echo "Tag $tag resolves to $release_commit, but origin/main is $main_commit" >&2
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Release checkout is not clean" >&2
    exit 1
fi

repository=${GITHUB_REPOSITORY:-}
if [[ ! "$repository" =~ ^[^/]+/[^/]+$ ]]; then
    echo "GITHUB_REPOSITORY must identify the owner and repository" >&2
    exit 2
fi

protection=.github/main-branch-protection.json
if [[ ! -f "$protection" ]]; then
    echo "Required-check configuration is missing: $protection" >&2
    exit 1
fi

mapfile -t required_checks < <(jq -r '
    .required_status_checks.checks[]
    | [.context, (.app_id | tostring)]
    | @tsv
' "$protection")
if ((${#required_checks[@]} == 0)); then
    echo "No required checks are configured in $protection" >&2
    exit 1
fi

check_runs=
for attempt in 1 2 3; do
    if check_runs=$(gh api \
        -H "Accept: application/vnd.github+json" \
        "repos/$repository/commits/$release_commit/check-runs?filter=latest&per_page=100"); then
        break
    fi
    if ((attempt == 3)); then
        echo "Unable to retrieve check runs for $release_commit" >&2
        exit 1
    fi
    sleep $((attempt * 2))
done

for required in "${required_checks[@]}"; do
    IFS=$'\t' read -r context app_id <<<"$required"
    result=$(jq -r --arg context "$context" --argjson app_id "$app_id" '
        [.check_runs[] | select(.name == $context and .app.id == $app_id)]
        | sort_by(.started_at // "")
        | last
        | if . == null then ["missing", "missing"]
          else [.status, (.conclusion // "pending")]
          end
        | @tsv
    ' <<<"$check_runs")
    IFS=$'\t' read -r status conclusion <<<"$result"
    if [[ "$status" != completed || "$conclusion" != success ]]; then
        echo "Required check is not successful for $release_commit: $context ($status/$conclusion)" >&2
        exit 1
    fi
    echo "Required check passed: $context"
done

echo "Release candidate verified: ewf-image $package_version at $release_commit"
