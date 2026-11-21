// The Railway project behind 2-Top's online services (project
// two-top-matchbox, environment production), all of it in one file.
//
// Railway never reads this file during a deploy. `railway config plan`
// shows what it would change, `railway config apply` writes it into each
// service's settings, and every deploy after that builds from those
// settings. The recipe is in PLAYBOOK.md, "Deploying the services".
//
// Every service is uploaded from the repository root and names its own
// Dockerfile here. Never leave one to guess: a service with no path picks up
// the root Dockerfile, which builds ice_vendor (the first tape_drop deploy
// shipped exactly that).
import { defineRailway, github, preserve, project, service, volume } from "railway/iac";

export default defineRailway(() => {
  // The Matchbox signaling server: the public room every build dials.
  const matchbox = service("two-top-matchbox", {
    build: { builder: "DOCKERFILE", dockerfilePath: "deploy/matchbox/Dockerfile" },
  });

  // Short-lived TURN credentials for phones behind carrier NAT
  // (SIGNALING.md). Its source is this repository on GitHub; the
  // Cloudflare key and the app key live only on Railway.
  const iceVendor = service("ice-vendor", {
    source: github("ampactor-labs/two-top"),
    build: { builder: "DOCKERFILE", dockerfilePath: "Dockerfile" },
    healthcheck: "/healthz",
    deploy: { restartPolicyType: "ALWAYS" },
    env: {
      CF_TURN_KEY_ID: preserve(),
      CF_TURN_API_TOKEN: preserve(),
      ICE_APP_KEY: preserve(),
    },
  });

  // The tape relay: shared tapes, live tapes, the ladder's results. Sealed
  // tapes are written under TAPE_DIR, which is this volume, so a redeploy
  // or a crash keeps every link alive for its TTL.
  const tapes = volume("tape-drop-volume", { region: "us-east4-eqdc4a", sizeMB: 5000 });
  const tapeDrop = service("tape-drop", {
    build: { builder: "DOCKERFILE", dockerfilePath: "crates/tape_drop/Dockerfile" },
    healthcheck: "/healthz",
    deploy: { restartPolicyType: "ALWAYS" },
    volumeMounts: { "/data": tapes },
    env: { TAPE_DIR: "/data" },
  });

  return project("two-top-matchbox", {
    resources: [matchbox, iceVendor, tapeDrop, tapes],
  });
});
