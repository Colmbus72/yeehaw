import { useState, useEffect } from 'react';
import { watch } from 'chokidar';
import type { Config, Project, Barn } from '../types.js';
import { loadConfig, loadProjects, loadBarns, loadProject } from '../lib/config.js';
import { YEEHAW_DIR } from '../lib/paths.js';

interface UseConfigReturn {
  config: Config;
  projects: Project[];
  barns: Barn[];
  currentProject: Project | null;
  setCurrentProjectName: (name: string | null) => void;
  reload: () => void;
}

export function useConfig(): UseConfigReturn {
  const [config, setConfig] = useState<Config>(() => loadConfig());
  const [projects, setProjects] = useState<Project[]>(() => loadProjects());
  const [barns, setBarns] = useState<Barn[]>(() => loadBarns());
  const [currentProjectName, setCurrentProjectName] = useState<string | null>(
    () => loadConfig().default_project
  );

  const reload = () => {
    setConfig(loadConfig());
    setProjects(loadProjects());
    setBarns(loadBarns());
  };

  useEffect(() => {
    const watcher = watch(YEEHAW_DIR, {
      ignoreInitial: true,
      depth: 2,
    });

    watcher.on('all', () => {
      reload();
    });

    return () => {
      watcher.close();
    };
  }, []);

  const currentProject = currentProjectName ? loadProject(currentProjectName) : null;

  return {
    config,
    projects,
    barns,
    currentProject,
    setCurrentProjectName,
    reload,
  };
}
