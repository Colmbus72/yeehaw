<div align="center">

```
             ██╗   ██╗███████╗███████╗██╗  ██╗ █████╗ ██╗    ██╗
 ░ ░▒░ ░▒░   ╚██╗ ██╔╝██╔════╝██╔════╝██║  ██║██╔══██╗██║    ██║
░▒ · ‿ · ▒░   ╚████╔╝ █████╗  █████╗  ███████║███████║██║ █╗ ██║
▒░ ▒░▒░ ░▒     ╚██╔╝  ██╔══╝  ██╔══╝  ██╔══██║██╔══██║██║███╗██║
 ░▒░ ░▒░ ░      ██║   ███████╗███████╗██║  ██║██║  ██║╚███╔███╔╝
                ╚═╝   ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝
```

A terminal dashboard for managing projects, servers, and deployments.

</div>

## Quick Start

```bash
npm install -g @colmbus72/yeehaw
yeehaw
```

**Requirements:** Node.js 20+, tmux

Config lives in `~/.yeehaw/`. Press `n` from the dashboard to create your first project or barn.

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

**Projects**       Organize codebases with per-project wikis and deployment tracking

**Herds**          Group related livestock and critters across barns


## MCP Server

Yeehaw includes an MCP server. Claude sessions started from Yeehaw have access to your projects, servers, logs, env files, and wikis.

```
"What errors are in the production logs?"
"Compare staging and local env variables"
"Which barn is the demo site on?"
```

## Configuration

<details>
<summary>Manual YAML setup</summary>

**Project** (`~/.yeehaw/projects/myapp.yaml`):
```yaml
name: myapp
path: ~/Code/myapp
summary: My web application
livestock:
  - name: local
    path: ~/Code/myapp
  - name: production
    path: /var/www/myapp
    barn: prod-server
```

**Barn** (`~/.yeehaw/barns/prod-server.yaml`):
```yaml
name: prod-server
host: myserver.com
user: deploy
port: 22
identity_file: ~/.ssh/id_rsa
```

</details>

## Development

```bash
npm install
npm run dev      # development mode with hot reload
npm run build    # build
npm run typecheck
```

Built with [Ink](https://github.com/vadimdemedes/ink), TypeScript, tmux, and MCP.

## License

MIT
