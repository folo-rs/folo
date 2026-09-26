# Choosing versions together

An assessment answers **whether released content changed**. A semantic decision
answers **what that change means to consumers**. A version plan translates the
decision into numeric versions and their workspace effects.

## Semantic decisions and numeric increments

The author supplies semantic decisions:

| Decision | Use |
| --- | --- |
| `breaking` | A change breaks a consumer promise. |
| `nonbreaking` | A compatible addition extends the consumer contract. |
| `patch` | A correction or other change needs no stronger semantic level. |

Consumer promises include documented behavior, CLI arguments, data formats,
feature availability and build requirements, not just Rust function signatures.

Numeric plan increments use another vocabulary: `patch`, `minor`, `major`.
These are not interchangeable with semantic decisions even though both JSON
formats call the field `level`.

| Starting version | Semantic decision | Example resulting version | Numeric increment |
| --- | --- | --- | --- |
| `1.4.0` | `nonbreaking` | `1.5.0` | `minor` |
| `1.4.0` | `breaking` | `2.0.0` | `major` |
| `0.4.2` | `nonbreaking` | `0.4.3` | `patch` |
| `0.4.2` | `breaking` | `0.5.0` | `minor` |

These examples start without an adequate pending increment or other group
effects. Proposal generation retains an already sufficient version rather than
incrementing it again.

The translation follows Cargo compatibility: the leftmost nonzero version
component defines the incompatible boundary. A `0.0.z` package has no compatible
next version. Explicit target versions are also supported when numeric increments
do not express the intended release.

## External API evidence is a floor

[`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks) detects
supported Rust API incompatibilities. Establish that the checker can run before
using its result: a rustdoc-format failure is not a compatibility pass.

Compare public library contracts with all features, but review feature changes
manually too. Moving an existing item behind a new feature can break callers
using a subset even when both all-features APIs contain the item. Removing a
feature, changing observable behavior or breaking a CLI can also require a
stronger decision than the checker reports.

Raise the decision when the contract requires it. Do not lower it below a valid
compatibility finding.

## Version groups

An exact local dependency such as `version = "=1.4.0"` joins its Git-tracked
workspace endpoints into a **version group**. Connected exact relationships,
in either dependency direction, make one group. There is no separate group
configuration.

Groups include normal, build and development dependencies, optional and
target-specific declarations, and inherited workspace dependencies. A registry
dependency, an outside-workspace path, a versionless path or an unused workspace
entry does not create a group.

Use a single exact `=major.minor.patch` requirement. Partial exact versions,
prerelease/build suffixes and compound exact requirements do not declare valid
workspace groups.

Every member shares the resolved version. The group includes `publish = false`
members: their versions can move for alignment, but their source changes receive
no semantic decision and they are never uploaded. The highest declared member
version participates in choosing the result, so alignment does not lower a member.

The running example is:

```text
widget             public library             1.4.0
  exact dependency on widget_impl
widget_impl        private implementation     1.4.0
widget-fixtures    publish = false helper      1.4.0
  exact dependency on widget_impl

widget-cli         separate binary package    2.0.0
  ordinary dependency on widget
  executable name: widget
```

The library, implementation and helper form one group. `widget-cli` does not:
ordinary compatible dependency requirements do not make exact-version groups.

## Private APIs and public dependencies

A published implementation package can declare:

```toml
[package.metadata.release-plan]
private-api = true
```

That means its library surface serves another package rather than consumers
directly. It is not the same as `publish = false`. It still has released content,
version-group obligations and dependency effects.

When `widget_impl` changes, compatibility selection reaches the public `widget`
contract through their group. Re-exported items are compared where consumers use
them, rather than demanding compatibility for every private implementation item.

A **public dependency** supplies types exposed by a dependent's public API.
Its incompatible version movement requires an incompatible release of the
dependent too. This is based on the version relationship, not a guess that the
particular exposed types were unaffected.

Exposure comes from each package's externally verified
`allowed_external_types` declarations. Defining crate names can differ from the
direct dependency through which a type is re-exported; the declarations are
followed transitively to attribute that dependency. Only normal dependencies
supply public API types.

Private implementation packages do not interrupt breaking-change propagation.
A re-exported type can expose another dependency through its methods without
the outer library's allow-list enumerating every such type. Conservatively
propagating through the implementation package preserves that consumer contract.
It can move a whole group even when one public member is unaffected.

This model requires a real
[external-type validation gate](../integration/repository.md#verify-external-type-exposure).
Ordinary `check` does not verify those declarations against compiled APIs.

## Requirement rewrites also matter

Workspace requirements must name the version their target declares, not merely
admit it within a broad range. An ordinary requirement such as `1.4.0` and an
exact requirement such as `=1.4.0` have different grouping effects, but both need
updating when their target moves.

A dependent manifest rewrite is released content. Preview accounts for that
rewrite and any binary lockfile effects, retaining sufficient pending increments.
The complete release set is therefore larger than the packages whose source the
author initially edited.
