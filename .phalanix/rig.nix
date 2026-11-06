# The mathfmt rig: seats that develop mathfmt, in the loop that built
# phalanix v0.6.0. Bring it up from a checkout with
#
#   phalanix up .phalanix/rig.nix
#
# `up` sends this one file to compute, the seat host, and evaluates it there
# (restricted: no imports, no reads), passing `phx` (phalanix's nix/phx.nix).
#
# Standing:
#
#   lead      one wave at a time: its spec, its plan and its ledger of
#             rulings (docs/handoff/wave-NN.md); starts task, review and
#             question seats with briefs, reads their summaries, chains
#             tasks with --base, and tells the person what to land
#
# Templates, each spawned by lead (or the person) with a brief:
#
#   task      one task of the plan, test first, on its own branch
#   review    an adversarial review of one branch against its base;
#             reports findings and never fixes them
#   question  one question about the code, answered
#
# Every kind runs on the strongest model, the person's rule for multi-agent
# work. Roles are repo: layers read from each seat's branch at every start,
# so .phalanix/ must be on main before the first seat starts. CLAUDE.md at
# the root is every seat's memory too: it holds the build, the tests and the
# person's rules, and the roles do not repeat it.
#
# The repository is jack/mathfmt on Forgejo, which is private: compute's
# daemon clones it and pushes submitted branches with the deploy key compute
# holds (secrets/phalanix-forge-forgejo in ~/dev/nix), which must be
# registered on the repository with write access. compute admits every seat
# user to its main nix daemon (nix.settings.allowed-users in ~/dev/nix). The
# dev shell vendors every crate from Cargo.lock (flake.nix, tests/dev.sh), so
# the seat proxy needs no crates.io entry: the nix daemon fetches the crates,
# and github.com with codeload (already allowed) serves the pinned nixpkgs
# when compute's store lacks it. Needs phalanix v0.6.0 or later.
{ host ? { }, lib, phx, ... }:
let
  repo = "ssh://forgejo@git.buttermilks.rocks/jack/mathfmt.git";
  layer = path: "repo:.phalanix/roles/${path}.md";

  # What every seat and template shares, given as each kind's own options.
  common = {
    inherit repo;
    tool = "claude";
    base = "main";
    continuity = "resume";
    model = "opus";
  };
  kind = traits: own: phx.kind traits (common // own);

  # A seat's PATH may have no nix or bash; the roles and CLAUDE.md spell it out.
  nix = "/run/current-system/sw/bin/nix";
  bash = "/run/current-system/sw/bin/bash";

  traits = {
    reports = phx.trait { role = layer "traits/reports"; };
    # The dev shell through tests/dev.sh (cargo against the vendored crates,
    # clippy, rustfmt; the wrapper drops the seat's identity first), and the
    # flake checks, which the nix daemon builds. This is a blanket
    # pre-approval: anything wrapped in dev.sh skips the permission
    # classifier, and dev.sh is a file the seat may edit on its own branch.
    # Accepted, because building and testing is the seat's whole job. The
    # second form is for a seat whose PATH has no bash for dev.sh's shebang.
    builds = phx.trait {
      permissions.allow = [
        "Bash(repo/tests/dev.sh *)"
        "Bash(${bash} repo/tests/dev.sh *)"
        "Bash(${nix} build *)"
        "Bash(${nix} flake check *)"
      ];
    };
  };

  rig = phx.merge [
    {
      seats = {
        # lead reads summaries, not logs, so it does not build.
        lead = kind [ ] {
          role = layer "lead";
          memory = "8G";
          accept = "ask";
          spawns = { task = 3; review = 2; question = 2; };
        };
      };
      templates = {
        task = kind [ traits.reports traits.builds ] {
          role = layer "task";
          memory = "8G";
          accept = "auto";
          spawns.question = 1;
        };
        review = kind [ traits.reports traits.builds ] {
          role = layer "review";
          memory = "8G";
          accept = "auto";
        };
        question = kind [ traits.reports traits.builds ] {
          role = layer "question";
          memory = "8G";
          accept = "auto";
        };
      };
    }
  ];

  kinds = rig.seats // rig.templates;
  both = a: b: [{ from = a; to = b; } { from = b; to = a; }];
in
rig // {
  name = "mathfmt";

  # Every seat: the verbs, and git in its clone (./repo).
  permissions.allow = [ "Bash(phalanix *)" "Bash(git -C repo *)" ];

  # lead talks to every other kind, both ways. A parent and its child reach
  # each other without an edge, so a task's question needs none.
  edges = rig.edges
    ++ lib.concatMap (both "lead") (lib.attrNames (removeAttrs kinds [ "lead" ]));

  commit.author_email = "mathfmt@phalanix.invalid";
}
