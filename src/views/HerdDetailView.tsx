import React, { useState, useMemo } from 'react';
import { Box, Text, useInput } from 'ink';
import { HerdHeader } from '../components/HerdHeader.js';
import { Panel } from '../components/Panel.js';
import { List, type ListItem } from '../components/List.js';
import type { Project, Barn, Herd, Livestock, Critter, RanchHand } from '../types.js';

type FocusedPanel = 'livestock' | 'critters';
type Mode =
  | 'normal'
  | 'add-livestock'
  | 'add-critter-barn'
  | 'add-critter-select'
  | 'remove-livestock-confirm'
  | 'remove-critter-confirm';

interface HerdDetailViewProps {
  project: Project;
  herd: Herd;
  barns: Barn[];
  ranchHands: RanchHand[];
  onBack: () => void;
  onUpdateHerd: (herd: Herd) => void;
  onSelectLivestock?: (livestock: Livestock, barn: Barn | null) => void;
  onSelectCritter?: (critter: Critter, barn: Barn) => void;
}

export function HerdDetailView({
  project,
  herd,
  barns,
  ranchHands,
  onBack,
  onUpdateHerd,
  onSelectLivestock,
  onSelectCritter,
}: HerdDetailViewProps) {
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('livestock');
  const [mode, setMode] = useState<Mode>('normal');
  const [selectedLivestockIndex, setSelectedLivestockIndex] = useState(0);
  const [selectedCritterIndex, setSelectedCritterIndex] = useState(0);

  // Add livestock flow
  const [selectedAddLivestockIndex, setSelectedAddLivestockIndex] = useState(0);

  // Add critter flow
  const [selectedBarnForCritter, setSelectedBarnForCritter] = useState<Barn | null>(null);
  const [selectedAddBarnIndex, setSelectedAddBarnIndex] = useState(0);
  const [selectedAddCritterIndex, setSelectedAddCritterIndex] = useState(0);

  // Remove confirmation
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [removeCritterTarget, setRemoveCritterTarget] = useState<{ barn: string; critter: string } | null>(null);

  // Find ranch hand that syncs this herd
  const syncingRanchHand = useMemo(() => {
    return ranchHands.find(rh => rh.herd === herd.name);
  }, [ranchHands, herd.name]);

  // Get livestock that are available to add (not in any herd)
  const availableLivestock = useMemo(() => {
    const allHerdLivestock = new Set<string>();
    for (const h of project.herds || []) {
      for (const l of h.livestock) {
        allHerdLivestock.add(l);
      }
    }
    return (project.livestock || []).filter((l) => !allHerdLivestock.has(l.name));
  }, [project]);

  // Get barns that have critters not yet in this herd
  const barnsWithAvailableCritters = useMemo(() => {
    return barns.filter((barn) => {
      const critters = barn.critters || [];
      return critters.some((c) => {
        return !herd.critters.some(
          (hc) => hc.barn === barn.name && hc.critter === c.name
        );
      });
    });
  }, [barns, herd.critters]);

  // Get available critters for selected barn
  const availableCrittersForBarn = useMemo(() => {
    if (!selectedBarnForCritter) return [];
    const barnCritters = selectedBarnForCritter.critters || [];
    return barnCritters.filter((c) => {
      return !herd.critters.some(
        (hc) => hc.barn === selectedBarnForCritter.name && hc.critter === c.name
      );
    });
  }, [selectedBarnForCritter, herd.critters]);

  // Build list items for livestock in herd
  const livestockItems: ListItem[] = herd.livestock.map((name) => {
    const livestock = (project.livestock || []).find((l) => l.name === name);
    const barnName = livestock?.barn || 'local';
    return {
      id: name,
      label: name,
      status: 'active',
      meta: barnName,
    };
  });

  // Build list items for critters in herd
  const critterItems: ListItem[] = herd.critters.map((ref) => ({
    id: `${ref.barn}:${ref.critter}`,
    label: ref.critter,
    status: 'active',
    meta: ref.barn,
  }));

  // Derive which barns are involved
  const derivedBarns = useMemo(() => {
    const barnSet = new Set<string>();
    for (const name of herd.livestock) {
      const livestock = (project.livestock || []).find((l) => l.name === name);
      barnSet.add(livestock?.barn || 'local');
    }
    for (const ref of herd.critters) {
      barnSet.add(ref.barn);
    }
    return Array.from(barnSet).sort();
  }, [herd, project.livestock]);

  useInput((input, key) => {
    // Handle escape
    if (key.escape) {
      if (mode !== 'normal') {
        setMode('normal');
        setRemoveTarget(null);
        setRemoveCritterTarget(null);
        setSelectedBarnForCritter(null);
      } else {
        onBack();
      }
      return;
    }

    // Handle remove confirmations
    if (mode === 'remove-livestock-confirm' && removeTarget) {
      if (input === 'y') {
        const updatedHerd: Herd = {
          ...herd,
          livestock: herd.livestock.filter((l) => l !== removeTarget),
        };
        onUpdateHerd(updatedHerd);
        setRemoveTarget(null);
        setMode('normal');
        if (selectedLivestockIndex >= updatedHerd.livestock.length && updatedHerd.livestock.length > 0) {
          setSelectedLivestockIndex(updatedHerd.livestock.length - 1);
        }
      } else if (input === 'n') {
        setRemoveTarget(null);
        setMode('normal');
      }
      return;
    }

    if (mode === 'remove-critter-confirm' && removeCritterTarget) {
      if (input === 'y') {
        const updatedHerd: Herd = {
          ...herd,
          critters: herd.critters.filter(
            (c) => !(c.barn === removeCritterTarget.barn && c.critter === removeCritterTarget.critter)
          ),
        };
        onUpdateHerd(updatedHerd);
        setRemoveCritterTarget(null);
        setMode('normal');
        if (selectedCritterIndex >= updatedHerd.critters.length && updatedHerd.critters.length > 0) {
          setSelectedCritterIndex(updatedHerd.critters.length - 1);
        }
      } else if (input === 'n') {
        setRemoveCritterTarget(null);
        setMode('normal');
      }
      return;
    }

    if (mode !== 'normal') return;

    // Tab to switch panels
    if (key.tab) {
      setFocusedPanel((p) => p === 'livestock' ? 'critters' : 'livestock');
      return;
    }

    // Add livestock
    if (input === 'n' && focusedPanel === 'livestock') {
      if (availableLivestock.length > 0) {
        setSelectedAddLivestockIndex(0);
        setMode('add-livestock');
      }
      return;
    }

    // Add critter
    if (input === 'n' && focusedPanel === 'critters') {
      if (barnsWithAvailableCritters.length > 0) {
        setSelectedAddBarnIndex(0);
        setSelectedBarnForCritter(null);
        setMode('add-critter-barn');
      }
      return;
    }

    // Delete livestock
    if (input === 'd' && focusedPanel === 'livestock') {
      if (herd.livestock.length > 0 && selectedLivestockIndex < herd.livestock.length) {
        setRemoveTarget(herd.livestock[selectedLivestockIndex]);
        setMode('remove-livestock-confirm');
      }
      return;
    }

    // Delete critter
    if (input === 'd' && focusedPanel === 'critters') {
      if (herd.critters.length > 0 && selectedCritterIndex < herd.critters.length) {
        const target = herd.critters[selectedCritterIndex];
        setRemoveCritterTarget({ barn: target.barn, critter: target.critter });
        setMode('remove-critter-confirm');
      }
      return;
    }
  });

  // Handle livestock selection for navigation
  const handleLivestockSelect = (item: ListItem) => {
    if (!onSelectLivestock) return;
    const livestock = (project.livestock || []).find((l) => l.name === item.id);
    if (livestock) {
      const barn = livestock.barn ? barns.find((b) => b.name === livestock.barn) || null : null;
      onSelectLivestock(livestock, barn);
    }
  };

  // Handle critter selection for navigation
  const handleCritterSelect = (item: ListItem) => {
    if (!onSelectCritter) return;
    const [barnName, critterName] = item.id.split(':');
    const barn = barns.find((b) => b.name === barnName);
    if (barn) {
      const critter = (barn.critters || []).find((c) => c.name === critterName);
      if (critter) {
        onSelectCritter(critter, barn);
      }
    }
  };

  // Add livestock selection screen
  if (mode === 'add-livestock') {
    const items: ListItem[] = availableLivestock.map((l) => ({
      id: l.name,
      label: l.name,
      status: 'active',
      meta: l.barn || 'local',
    }));

    return (
      <Box flexDirection="column" flexGrow={1}>
        <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Select livestock to add</Text>
          <Text dimColor>These livestock are not in any herd</Text>
          <Box marginTop={1} flexDirection="column">
            <List
              items={items}
              focused={true}
              selectedIndex={selectedAddLivestockIndex}
              onSelectionChange={setSelectedAddLivestockIndex}
              onSelect={(item) => {
                const updatedHerd: Herd = {
                  ...herd,
                  livestock: [...herd.livestock, item.id],
                };
                onUpdateHerd(updatedHerd);
                setMode('normal');
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter - select barn first
  if (mode === 'add-critter-barn') {
    const items: ListItem[] = barnsWithAvailableCritters.map((b) => ({
      id: b.name,
      label: b.name,
      status: 'active',
      meta: b.host || 'local',
    }));

    return (
      <Box flexDirection="column" flexGrow={1}>
        <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Select barn</Text>
          <Text dimColor>Choose which barn has the critter</Text>
          <Box marginTop={1} flexDirection="column">
            <List
              items={items}
              focused={true}
              selectedIndex={selectedAddBarnIndex}
              onSelectionChange={setSelectedAddBarnIndex}
              onSelect={(item) => {
                const barn = barns.find((b) => b.name === item.id);
                if (barn) {
                  setSelectedBarnForCritter(barn);
                  setSelectedAddCritterIndex(0);
                  setMode('add-critter-select');
                }
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Add critter - select critter from barn
  if (mode === 'add-critter-select' && selectedBarnForCritter) {
    const items: ListItem[] = availableCrittersForBarn.map((c) => ({
      id: c.name,
      label: c.name,
      status: 'active',
      meta: c.service,
    }));

    return (
      <Box flexDirection="column" flexGrow={1}>
        <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="green">Select critter</Text>
          <Text dimColor>From barn: {selectedBarnForCritter.name}</Text>
          <Box marginTop={1} flexDirection="column">
            <List
              items={items}
              focused={true}
              selectedIndex={selectedAddCritterIndex}
              onSelectionChange={setSelectedAddCritterIndex}
              onSelect={(item) => {
                const updatedHerd: Herd = {
                  ...herd,
                  critters: [
                    ...herd.critters,
                    { barn: selectedBarnForCritter.name, critter: item.id },
                  ],
                };
                onUpdateHerd(updatedHerd);
                setSelectedBarnForCritter(null);
                setMode('normal');
              }}
            />
          </Box>
          <Box marginTop={1}>
            <Text dimColor>Enter: select, Esc: cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Remove livestock confirmation
  if (mode === 'remove-livestock-confirm' && removeTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Livestock</Text>
          <Box marginTop={1}>
            <Text>Remove "{removeTarget}" from this herd?</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Remove critter confirmation
  if (mode === 'remove-critter-confirm' && removeCritterTarget) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />
        <Box flexDirection="column" padding={2}>
          <Text bold color="red">Remove Critter</Text>
          <Box marginTop={1}>
            <Text>Remove "{removeCritterTarget.critter}" ({removeCritterTarget.barn}) from this herd?</Text>
          </Box>
          <Box marginTop={1} gap={2}>
            <Text color="red" bold>[y] Yes, remove</Text>
            <Text dimColor>[n/Esc] Cancel</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Normal view - header with ASCII art + two panels
  const livestockHints = '[n] add  [d] remove';
  const critterHints = '[n] add  [d] remove';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <HerdHeader herd={herd} projectColor={project.color} ranchHandName={syncingRanchHand?.name} />

      {/* Info line with barns involved */}
      <Box paddingX={2} marginBottom={1}>
        {derivedBarns.length > 0 && (
          <Text>
            <Text dimColor>barns:</Text> {derivedBarns.join(', ')}
          </Text>
        )}
      </Box>

      {/* Two panels: Livestock and Critters */}
      <Box flexGrow={1} paddingX={1} gap={2}>
        {/* Left: Livestock */}
        <Panel
          title="Livestock"
          focused={focusedPanel === 'livestock'}
          width="50%"
          hints={livestockHints}
        >
          {livestockItems.length > 0 ? (
            <List
              items={livestockItems}
              focused={focusedPanel === 'livestock'}
              selectedIndex={selectedLivestockIndex}
              onSelectionChange={setSelectedLivestockIndex}
              onSelect={handleLivestockSelect}
            />
          ) : (
            <Text dimColor>No livestock in this herd</Text>
          )}
        </Panel>

        {/* Right: Critters */}
        <Panel
          title="Critters"
          focused={focusedPanel === 'critters'}
          width="50%"
          hints={critterHints}
        >
          {critterItems.length > 0 ? (
            <List
              items={critterItems}
              focused={focusedPanel === 'critters'}
              selectedIndex={selectedCritterIndex}
              onSelectionChange={setSelectedCritterIndex}
              onSelect={handleCritterSelect}
            />
          ) : (
            <Text dimColor>No critters in this herd</Text>
          )}
        </Panel>
      </Box>
    </Box>
  );
}
