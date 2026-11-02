"""Manually retire Abox GHCR versions while retaining deployed/latest OCI graphs."""
import hashlib
import json
import os
import re
from pathlib import Path
from urllib.request import Request, urlopen

PACKAGE = "wangzexi/abox"
API = "https://api.github.com/users/wangzexi/packages/container/abox/versions"
ACCEPT = ", ".join((
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.docker.distribution.manifest.v2+json",
))


def request(url, headers, method="GET"):
    with urlopen(Request(url, headers=headers, method=method), timeout=60) as response:
        return response.read(), response.headers


def manifest_closure(roots, load):
    retained = set()
    pending = list(roots)
    while pending:
        digest = pending.pop()
        if digest in retained:
            continue
        manifest = load(digest)
        retained.add(digest)
        pending.extend(item["digest"] for item in manifest.get("manifests", []))
        if manifest.get("subject"):
            pending.append(manifest["subject"]["digest"])
    return retained


def main():
    assert os.environ["GITHUB_REPOSITORY"] == PACKAGE
    deployed = os.environ["DEPLOYED_DIGEST"]
    assert re.fullmatch(r"sha256:[0-9a-f]{64}", deployed)
    apply = os.environ.get("APPLY", "false").lower() == "true"
    api_headers = {
        "Authorization": "Bearer " + os.environ["GH_TOKEN"],
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    token_body, _ = request(
        "https://ghcr.io/token?service=ghcr.io&scope=repository:" + PACKAGE + ":pull", {}
    )
    registry_headers = {
        "Authorization": "Bearer " + json.loads(token_body)["token"],
        "Accept": ACCEPT,
    }

    def load(reference):
        body, _ = request("https://ghcr.io/v2/" + PACKAGE + "/manifests/" + reference, registry_headers)
        if reference.startswith("sha256:"):
            assert "sha256:" + hashlib.sha256(body).hexdigest() == reference
        return json.loads(body)

    latest_body, latest_headers = request(
        "https://ghcr.io/v2/" + PACKAGE + "/manifests/latest", registry_headers
    )
    latest = latest_headers.get("Docker-Content-Digest") or "sha256:" + hashlib.sha256(latest_body).hexdigest()
    roots = {deployed, latest}
    retained = manifest_closure(roots, load)
    versions = []
    page = 1
    while True:
        body, _ = request(API + f"?per_page=100&page={page}", api_headers)
        batch = json.loads(body)
        if not batch:
            break
        versions.extend(batch)
        page += 1
    assert versions and all(re.fullmatch(r"sha256:[0-9a-f]{64}", v["name"]) for v in versions)
    assert roots <= {v["name"] for v in versions}
    eligible = [v for v in versions if v["name"] not in retained]
    summary = {"total": len(versions), "kept": len(versions) - len(eligible), "eligible": len(eligible), "apply": apply, "retained_roots": sorted(roots)}
    print(json.dumps(summary), flush=True)
    if apply:
        for version in eligible:
            request(API + "/" + str(version["id"]), api_headers, "DELETE")
        # Confirm the retained index, platform and attestation manifests remain retrievable.
        manifest_closure(roots, load)
        print(json.dumps({"deleted": len(eligible), "retained_graph_verified": True}), flush=True)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        Path(os.environ["GITHUB_STEP_SUMMARY"]).write_text("```json\n" + json.dumps(summary, indent=2) + "\n```\n")


if __name__ == "__main__":
    main()
