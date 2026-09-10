<div align="center">

```
             ██╗   ██╗███████╗███████╗██╗  ██╗ █████╗ ██╗    ██╗
 ░ ░▒░ ░▒░   ╚██╗ ██╔╝██╔════╝██╔════╝██║  ██║██╔══██╗██║    ██║
░▒ · ‿ · ▒░   ╚████╔╝ █████╗  █████╗  ███████║███████║██║ █╗ ██║
▒░ ▒░▒░ ░▒     ╚██╔╝  ██╔══╝  ██╔══╝  ██╔══██║██╔══██║██║███╗██║
 ░▒░ ░▒░ ░      ██║   ███████╗███████╗██║  ██║██║  ██║╚███╔███╔╝
                ╚═╝   ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝
```

A terminal dashboard for managing projects, servers, and Claude sessions.

</div>

## Quick Start

```bash
curl -LsSf https://yeehaw.cool/install.sh | sh    # or: brew install Colmbus72/yeehaw/yeehaw
yeehaw
```

**Requirements:** tmux. On Linux, build from source: `cargo install --git https://github.com/Colmbus72/yeehaw`

Config lives in `~/.yeehaw/`. Press `n` from the dashboard to create your first project or barn, then `c` to start Claude in it. Claude sessions started from Yeehaw get its MCP server automatically. To add it elsewhere: `claude mcp add yeehaw -- yeehaw mcp-server`.

## Features

```
       /;    ;\
   __  \\____//
  /{_\_/   `'\___
  \___   (o)  (o }
    /          :-'  Livestock - Deployed instances of your repository (local, staging, prod)
      \_    `__\\
        \___(o'o)
        (  `===='
```

```
              __
     /\    .-" /
    /  ; .'  .'
   :   :/  .'
    \  ;-.'
  --..__/ `.        Critters - System services (MySQL, Redis, NGINX). View logs via journalctl.
  .'     o  \
             ;
    \       :
     `.__.--'
```

```
      _.-^-._
   .-'   _   '-.
  /     |_|     \
 /               \
/|     _____     |\ Barns - Servers (local or remote via SSH).
 |    |==|==|    |
 |    |--|--|    |
 |    |==|==|    |
```

```
       ,_.,
    __/ `_(__
   '-..,__..-`
     @ *Y*|
     |  - |         Ranch Hands - Sync infrastructure from Kubernetes or Terraform.
  ___'_..'.._
 /   \_\'/_| \
```

```
        _    _
      ('')  ('')
   ~~~(  )~~(  )~~  Worms - Scheduled jobs. Shell commands or Claude prompts on a cron.
```

```
    ______________
   |  TRAIL  ---> |
   |______________| Trails - Multi-step automations in GitHub Actions-style YAML.
         ||
   ^^^^^^^^^^^^^^^
```

**Projects**       Organize codebases with per-project wikis and deployment tracking

**Herds**          Group related livestock and critters across barns

## Keybindings

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `j` `k` | move | `n` | new |
| `Tab` | switch panel | `d` | delete |
| `Enter` | open | `c` | Claude / connect to barn |
| `Esc` | back | `s` | shell / SSH |
| `?` | help | `v` | session grid |
| `q` | detach | `Q` | quit |

From any window: `Ctrl+Y` dashboard · `Ctrl+H`/`Ctrl+L` prev/next window · `Ctrl+P` vault · `Ctrl+Q` leave remote barn

## License

MIT
