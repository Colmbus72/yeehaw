# Yeehaw: Herds & Future IaC Integration

## Overview

This document captures the conceptual model for extending Yeehaw with **Herds** (immediate next step) and **Ranch Hands** (future IaC integration), while preserving the simplicity that makes Yeehaw enjoyable to use.

The guiding principle: **protect what works, add incrementally.**

---

## Current Model

### Entities Today

```
Barn (physical server)
 ├── name, host, user, identity_file, port
 └── Critters[] (system services)
      └── name, service, log_path, config_path

Project (abstract grouping)
 └── Livestock[] (deployed apps)
      └── name, path, branch, env_path, log_path, barn reference
```

### How It Works Today

1. **Barn** = a physical server you can SSH into
2. **Critter** = a system service on a barn (mysql, redis, nginx)
3. **Project** = an abstract grouping that organizes livestock
4. **Livestock** = a deployed application on a barn (has repo, path, branch, etc.)

### Key Relationships

- Livestock belongs to a **Project** and lives on a **Barn**
- Critters belong to a **Barn** (not project-scoped)
- Projects don't have repos directly — **Livestock** have repos
- Projects organize livestock into a coherent "this is my product" view

### What's Missing

1. **No way to group livestock + critters that work together** (environments)
2. **No way to define connections** (which livestock talks to which critter)
3. **No path to IaC** (everything is manual)

---

## The Herd Concept

### What Is a Herd?

A **Herd** is a project-level grouping that defines which livestock and critters work together, and how they're connected.

```
Herd
 ├── name (e.g., "production", "staging", "client-a")
 ├── livestock[] (references to livestock in this project)
 ├── critters[] (references to critters on any barn)
 └── connections[] (which livestock uses which critter)
```

### Why "Herd"?

- Open-ended: can represent environments, client deployments, or any logical grouping
- Fits the metaphor: a herd is livestock that move/work together
- Doesn't force structure: users decide what herds mean to them

### The Updated Model

```
Barn (physical server)
 └── Critters[] (system services installed here)

Project (abstract grouping)
 ├── Livestock[] (deployed apps, each lives on a barn)
 └── Herds[] (groupings that define what works together)
      ├── livestock[] (which livestock are in this herd)
      ├── critters[] (which critters this herd uses)
      └── connections[] (which livestock talks to which critter)
```

### Rules

| Entity | Belongs To | Can Be In Multiple Herds? |
|--------|-----------|---------------------------|
| Barn | Global | N/A (herds don't contain barns) |
| Critter | Barn | Yes |
| Project | Global | N/A |
| Livestock | Project (lives on a Barn) | TBD (probably yes) |
| Herd | Project | N/A |

**Key insight:** Barns are derived, not assigned. You don't say "these barns are in this herd." You say "these livestock/critters are in this herd" and the barns are implied by where they live.

---

## Example: Ascend Training Project

### The Setup

```
Barn: "server-1" (app server)
 └── Critters: mysql, redis

Barn: "server-2" (worker server)
 └── Critters: (none)

Project: "Ascend Training"
 ├── Livestock:
 │    ├── api-prod (barn: server-1, path: /var/www/api)
 │    ├── api-staging (barn: server-1, path: /var/www/api-staging)
 │    ├── worker-prod (barn: server-2, path: /var/www/worker)
 │    └── worker-staging (barn: server-2, path: /var/www/worker-staging)
 │
 └── Herds:
      ├── "production"
      └── "staging"
```

### Production Herd

```
Herd: "production"
┌─────────────────────────────────────────────────┐
│  Livestock:                                     │
│   🐄 api-prod (server-1)                        │
│   🐄 worker-prod (server-2)                     │
│                                                 │
│  Critters:                                      │
│   🐿️ mysql (server-1)                           │
│   🐿️ redis (server-1)                           │
│                                                 │
│  Connections:                                   │
│   api-prod ──────→ mysql                        │
│   api-prod ──────→ redis                        │
│   worker-prod ───→ redis                        │
│                                                 │
│  Derived Barns: server-1, server-2              │
└─────────────────────────────────────────────────┘
```

### Staging Herd

```
Herd: "staging"
┌─────────────────────────────────────────────────┐
│  Livestock:                                     │
│   🐄 api-staging (server-1)                     │
│   🐄 worker-staging (server-2)                  │
│                                                 │
│  Critters:                                      │
│   🐿️ mysql (server-1)  ← same mysql as prod!   │
│   🐿️ redis (server-1)  ← same redis as prod!   │
│                                                 │
│  Connections:                                   │
│   api-staging ──────→ mysql                     │
│   api-staging ──────→ redis                     │
│   worker-staging ───→ redis                     │
│                                                 │
│  Derived Barns: server-1, server-2              │
└─────────────────────────────────────────────────┘
```

Note: The same critters (mysql, redis) are used by both herds. This is valid — critters can be referenced by multiple herds.

---

## Connection Model

### What Is a Connection?

A connection defines that a livestock uses a critter. At minimum:

```
Connection
 ├── livestock (reference)
 ├── critter (reference)
 └── metadata (optional: port, database name, etc.)
```

### Why Connections Matter

1. **Visibility:** See at a glance what your app depends on
2. **Documentation:** The herd IS the infrastructure diagram
3. **Future automation:** Ranch hands can auto-discover connections from env vars
4. **Debugging:** "My app is slow" → check the critters it connects to

### Connection Metadata (Future)

```
Connection:
  livestock: api-prod
  critter: mysql
  metadata:
    database: ascend_prod
    port: 3306
    env_var: DATABASE_URL
```

This could be auto-populated by reading the livestock's env file.

---

## Future: Ranch Hand (IaC Integration)

### What Is a Ranch Hand?

A **Ranch Hand** is an IaC provider that automatically populates barns, livestock, critters, and herds from an external source of truth (Kubernetes, Terraform, etc.).

```
Ranch Hand
 ├── name
 ├── type (kubernetes, terraform, manual)
 ├── config (cluster context, state file path, etc.)
 └── sync settings
```

### How It Works

```
                    ┌─────────────────┐
                    │   Kubernetes    │
                    │   Cluster       │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │   Ranch Hand    │
                    │   (k8s type)    │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
         ┌────────┐    ┌──────────┐   ┌────────┐
         │ Barns  │    │ Livestock│   │ Herds  │
         │(nodes) │    │(deploys) │   │(namespaces)
         └────────┘    └──────────┘   └────────┘
```

### Kubernetes Mapping

| Kubernetes | Yeehaw |
|------------|--------|
| Node | Barn |
| Deployment | Livestock |
| Pod | Livestock (see below) |
| Service (mysql, redis) | Critter |
| Namespace | Herd |
| Service connections | Connections |

In Kubernetes, a single livestock (deployment) runs as multiple pods across multiple nodes. This is represented as multiple livestock:

- **Manual livestock:** One cow, one barn. Simple.
- **K8s livestock:** Each pod is a livestock since thats all a livestock is a map of app to filepath on a server.

The livestock abstraction stays the same.

### Ranch Hand Doesn't Change the Model

The model (barns, livestock, critters, herds, connections) stays identical whether populated manually or by a ranch hand. The ranch hand just fills it automatically.

```
Manual user:     Adds barns, livestock, herds by hand
Ranch hand:      Syncs barns, livestock, herds from k8s/terraform
                         ↓
                 Same model, same UI, same experience
```

---

## Implementation Phases

### Phase 1: Herds (Immediate)

**Goal:** Add herds to the current manual-first model.

**What to build:**
- [ ] Herd entity (name, project reference)
- [ ] Livestock ↔ Herd association
- [ ] Critter ↔ Herd association (reference, not ownership)
- [ ] Connection entity (livestock → critter)
- [ ] UI: View/manage herds within a project
- [ ] UI: See derived barns for a herd
- [ ] UI: Visualize connections

**What doesn't change:**
- Barns work exactly as they do today
- Livestock work exactly as they do today
- Critters work exactly as they do today

### Phase 2: Connection Discovery (Later)

**Goal:** Auto-populate connections from livestock env files.

**What to build:**
- [ ] Parse livestock env files for known patterns (DATABASE_URL, REDIS_HOST, etc.)
- [ ] Suggest connections based on discovered values
- [ ] Match env values to critter hosts/ports

### Phase 3: Ranch Hand (Future)

**Goal:** Support IaC providers that auto-populate the model.

**What to build:**
- [ ] Ranch Hand entity (type, config)
- [ ] Kubernetes provider (sync nodes, deployments, services, namespaces)
- [ ] Terraform provider (future)
- [ ] Background sync / refresh mechanism

---

## Design Principles

1. **Protect what works.** The manual flow is enjoyable. Don't break it.

2. **Add incrementally.** Herds first, then connections, then ranch hands.

3. **Derive, don't duplicate.** Barns are derived from where livestock/critters live. Don't make users assign barns to herds.

4. **Open-ended naming.** Herds can be environments, clients, features — whatever makes sense to the user.

5. **Same model, different sources.** Whether you add things manually or via ranch hand, the data model is identical.

6. **Real-world metaphor.** Barns are physical. Livestock are deployed apps. Herds are groups that work together. Ranch hands manage the operation.

---

## Glossary

| Term | Definition |
|------|------------|
| **Barn** | A physical or virtual server you can SSH into |
| **Critter** | A system service running on a barn (mysql, redis, nginx) |
| **Project** | An abstract grouping that organizes livestock |
| **Livestock** | A deployed application on a barn |
| **Herd** | A project-level grouping of livestock + critters that work together |
| **Connection** | A defined relationship: this livestock uses this critter |
| **Ranch Hand** | An IaC provider that auto-populates barns, livestock, critters, and herds |

---

## Summary

**Today:** Barns, critters, projects, livestock — all manual, all simple.

**Next step:** Add herds to group livestock + critters and define connections.
**DONE**

**NEXT:** Add ranch hands to sync from Kubernetes/Terraform, introducing sync for pod-level detail.

The core model stays clean. The metaphor stays intact. The tummy stays happy.
