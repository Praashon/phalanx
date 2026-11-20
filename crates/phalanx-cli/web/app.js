(() => {
  'use strict';

  const SYSCALL_REF = [
    ['self_x()', '() -> fix', 2, 'Self X world position'],
    ['self_y()', '() -> fix', 2, 'Self Y world position'],
    ['self_vx()', '() -> fix', 2, 'Self X velocity'],
    ['self_vy()', '() -> fix', 2, 'Self Y velocity'],
    ['self_heading()', '() -> fix', 2, 'Current facing angle (rad)'],
    ['self_hp()', '() -> i32', 2, 'Remaining hit points (0..100)'],
    ['self_energy()', '() -> i32', 2, 'Available energy (0..100)'],
    ['self_id()', '() -> i32', 1, 'Arena bot slot index'],
    ['tick_num()', '() -> i32', 1, 'Current simulation tick'],
    ['arena_width()', '() -> fix', 1, 'Arena width (100.0)'],
    ['arena_height()', '() -> fix', 1, 'Arena height (60.0)'],
    ['alive_count()', '() -> i32', 2, 'Number of living bots'],
    ['zone_radius()', '() -> fix', 2, 'Current storm safe-zone radius'],
    ['zone_dist()', '() -> fix', 2, 'Distance from arena center'],
    ['radar_nearest()', '() -> i32', 6, 'Nearest living enemy ID, or -1'],
    ['target_dist(id)', '(i32) -> fix', 3, 'Euclidean distance to target'],
    ['target_angle(id)', '(i32) -> fix', 4, 'Bearing from self to target'],
    ['target_x(id)', '(i32) -> fix', 2, 'Target X coordinate'],
    ['target_y(id)', '(i32) -> fix', 2, 'Target Y coordinate'],
    ['target_vx(id)', '(i32) -> fix', 2, 'Target X velocity'],
    ['target_vy(id)', '(i32) -> fix', 2, 'Target Y velocity'],
    ['target_hp(id)', '(i32) -> i32', 2, 'Target hit points'],
    ['scan_cone(a, w, r)', '(fix, fix, fix) -> i32', 8, 'Nearest enemy in LOS cone'],
    ['nearest_bullet()', '() -> i32', 5, 'Nearest hostile projectile ID, or -1'],
    ['bullet_dist(id)', '(i32) -> fix', 3, 'Distance to projectile'],
    ['bullet_angle(id)', '(i32) -> fix', 4, 'Angle from self to projectile'],
    ['bullet_vx(id)', '(i32) -> fix', 2, 'Projectile X velocity'],
    ['bullet_vy(id)', '(i32) -> fix', 2, 'Projectile Y velocity'],
    ['wall_dist(angle)', '(fix) -> fix', 6, 'Raycast distance to wall or pillar'],
    ['thrust(angle, pwr)', '(fix, fix) -> void', 4, 'Apply thruster acceleration [0..1]'],
    ['fire(angle, lvl)', '(fix, i32) -> bool', 5, 'Fire projectile (power 1..3)'],
    ['shield(on)', '(bool) -> bool', 3, 'Toggle damage-mitigating shield'],
    ['dash(angle)', '(fix) -> bool', 5, 'Trigger directional dash impulse'],
    ['sin(a)', '(fix) -> fix', 3, 'Fixed-point sine'],
    ['cos(a)', '(fix) -> fix', 3, 'Fixed-point cosine'],
    ['atan2(y, x)', '(fix, fix) -> fix', 4, 'Fixed-point 2-arg arctangent'],
    ['sqrt(x)', '(fix) -> fix', 4, 'Fixed-point square root'],
    ['abs_fix(x)', '(fix) -> fix', 1, 'Fixed-point absolute value'],
    ['abs_int(x)', '(i32) -> i32', 1, 'Integer absolute value'],
    ['min_fix(a, b)', '(fix, fix) -> fix', 1, 'Minimum of two fix values'],
    ['max_fix(a, b)', '(fix, fix) -> fix', 1, 'Maximum of two fix values'],
    ['clamp_fix(x,lo,hi)', '(fix, fix, fix) -> fix', 2, 'Clamp x into [lo, hi]'],
    ['rand_int(max)', '(i32) -> i32', 2, 'Per-bot deterministic PRNG [0, max)'],
    ['debug_log(tag, v)', '(i32, fix) -> void', 2, 'Emit event log trace entry'],
    ['angle_diff(a, b)', '(fix, fix) -> fix', 2, 'Signed wrapped difference [-PI, PI]'],
  ];

  const app = {
    templates: [],
    rosterBots: [],
    match: null,
    cursor: 0,
    playFloat: 0,
    playing: true,
    speed: 1,
    selectedBot: 0,
    rosterRefs: [],
    eventCacheLen: -1,
    lastFrameTs: 0,
    dbgTrace: null,
    dbgStep: 0,
    activeFileIdx: 0,
    errorLines: new Set(),
    tourneyFormat: 'round-robin',
    runCount: 0,
  };

  const $ = id => document.getElementById(id);

  // Navigation tabs
  function switchTab(name) {
    document.querySelectorAll('.nav-tab').forEach(btn => {
      const on = btn.dataset.tab === name;
      btn.classList.toggle('active', on);
      btn.setAttribute('aria-selected', on ? 'true' : 'false');
    });
    document.querySelectorAll('.view').forEach(v => v.classList.toggle('active', v.id === 'tab-' + name));
    if (name === 'arena') resizeCanvas();
    if (name === 'editor') syncGutterScroll();
  }

  document.querySelectorAll('.nav-tab').forEach(btn => {
    btn.addEventListener('click', () => switchTab(btn.dataset.tab));
  });

  document.querySelectorAll('[data-sub]').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('[data-sub]').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      $('sub-events').classList.toggle('active', btn.dataset.sub === 'events');
      $('sub-vm').classList.toggle('active', btn.dataset.sub === 'vm');
    });
  });

  document.querySelectorAll('[data-edsub]').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('[data-edsub]').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      ['disasm', 'ir', 'syscalls'].forEach(k => {
        $('edsub-' + k).classList.toggle('active', btn.dataset.edsub === k);
      });
    });
  });

  // Canvas Setup
  const canvas = $('arenaCanvas');
  const ctx = canvas.getContext('2d');

  function resizeCanvas() {
    const parent = canvas.parentElement;
    if (!parent) return;
    const rect = parent.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(320, Math.floor(rect.width * dpr));
    canvas.height = Math.max(200, Math.floor(rect.height * dpr));
    renderCanvas();
  }
  window.addEventListener('resize', resizeCanvas);

  async function api(path, body) {
    const opts = body !== undefined
      ? { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }
      : { method: 'GET' };
    const res = await fetch(path, opts);
    return await res.json();
  }

  async function boot() {
    renderSyscallTable('');
    app.templates = await api('/api/bots');
    app.rosterBots = app.templates
      .filter(b => b.builtin_fighter)
      .map(b => ({ name: b.name, source: b.source }));

    buildFileTabs();
    loadTemplate(0);
    resizeCanvas();
    await runMatch(Number($('seedInput').value) || 42);
    requestAnimationFrame(tickLoop);
  }

  // ==================== ARENA ====================
  async function runMatch(seed) {
    $('statusReadout').textContent = `simulating seed ${seed}...`;
    $('seedInput').value = seed;
    const data = await api('/api/match', { seed, max_ticks: 1200, bots: app.rosterBots });
    if (!data.ok) {
      $('statusReadout').textContent = `compile error in ${data.error_bot}`;
      switchTab('editor');
      return;
    }

    app.match = data;
    app.cursor = 0;
    app.playFloat = 0;
    app.playing = true;
    app.dbgTrace = null;
    app.eventCacheLen = -1;
    syncPlayButton();

    $('gasCapLabel').textContent = `gas cap ${data.arena.gas_budget}`;
    $('verifyReadout').textContent = data.determinism_verified
      ? `${data.frames.length - 1} ticks verified (${(data.replay_bytes / 1024).toFixed(1)} KB .phxr)`
      : 'hash mismatch';
    $('verifyReadout').className = data.determinism_verified ? 'verify-ok' : 'verify-err';
    $('statusReadout').textContent = `seed ${seed} · ${data.determinism_verified ? 'verified' : 'unverified'}`;

    buildKillPips();
    buildRosterRows();
    updateArenaUI(true);
  }

  function buildKillPips() {
    const pips = $('timelinePips');
    pips.innerHTML = '';
    const frames = app.match.frames;
    const total = Math.max(1, frames.length - 1);
    frames.forEach((f, idx) => {
      for (const ev of f.events) {
        if (ev.kind === 'kill' || ev.kind === 'storm') {
          const el = document.createElement('div');
          el.className = 'timeline-pip';
          el.style.left = `${(idx / total) * 100}%`;
          el.title = ev.text;
          pips.appendChild(el);
        }
      }
    });
  }

  function buildRosterRows() {
    const wrap = $('rosterTable');
    wrap.innerHTML = '';
    app.rosterRefs = app.match.bot_summary.map((meta, idx) => {
      const row = document.createElement('div');
      row.className = 'roster-row';
      row.onclick = () => {
        app.selectedBot = idx;
        updateArenaUI(false);
      };

      const bar = document.createElement('div');
      bar.className = 'roster-bar';
      bar.style.background = meta.color;

      const nameWrap = document.createElement('div');
      nameWrap.className = 'roster-name-wrap';
      const nameEl = document.createElement('div');
      nameEl.className = 'roster-name';
      nameEl.textContent = meta.name;
      const subEl = document.createElement('div');
      subEl.className = 'roster-sub';
      nameWrap.append(nameEl, subEl);

      const meters = document.createElement('div');
      meters.className = 'roster-meters';

      const hpRow = document.createElement('div');
      hpRow.className = 'meter-row';
      const hpTrack = document.createElement('div');
      hpTrack.className = 'meter-track';
      const hpFill = document.createElement('div');
      hpFill.className = 'meter-fill';
      hpTrack.appendChild(hpFill);
      const hpVal = document.createElement('span');
      hpVal.style.minWidth = '24px';
      hpVal.style.textAlign = 'right';
      hpRow.append(hpTrack, hpVal);

      const enRow = document.createElement('div');
      enRow.className = 'meter-row';
      const enTrack = document.createElement('div');
      enTrack.className = 'meter-track';
      const enFill = document.createElement('div');
      enFill.className = 'meter-fill';
      enFill.style.background = '#38bdf8';
      enTrack.appendChild(enFill);
      const enVal = document.createElement('span');
      enVal.style.minWidth = '24px';
      enVal.style.textAlign = 'right';
      enRow.append(enTrack, enVal);

      meters.append(hpRow, enRow);

      const right = document.createElement('div');
      right.className = 'roster-right';
      const vmPill = document.createElement('span');
      vmPill.className = 'vm-pill';
      const gasEl = document.createElement('div');
      gasEl.className = 'muted';
      gasEl.style.fontSize = '10px';
      right.append(vmPill, gasEl);

      row.append(bar, nameWrap, meters, right);
      wrap.appendChild(row);
      return { row, subEl, hpFill, hpVal, enFill, enVal, vmPill, gasEl };
    });
  }

  function updateArenaUI(forceEvents) {
    const m = app.match;
    if (!m || !m.frames.length) return;
    const f = m.frames[app.cursor];
    const maxIdx = m.frames.length - 1;

    const alive = f.bots.filter(b => b.alive).length;
    $('hudTelemetry').textContent = `TICK ${String(f.t).padStart(4, '0')}/${String(maxIdx).padStart(4, '0')} · ZONE ${f.zr.toFixed(1)}u · ${alive} ALIVE`;
    $('hudStatus').textContent = app.cursor >= maxIdx
      ? (m.winner ? `WINNER ${m.winner.toUpperCase()}` : 'DRAW')
      : (app.playing ? 'RUNNING' : 'PAUSED');

    const pct = maxIdx > 0 ? (app.cursor / maxIdx) * 100 : 0;
    $('timelineFill').style.width = `${pct}%`;
    $('timelineThumb').style.left = `${pct}%`;
    $('timelineLabel').textContent = `${f.t} / ${maxIdx}`;
    $('hashValue').textContent = f.hash;

    f.bots.forEach((b, idx) => {
      const r = app.rosterRefs[idx];
      if (!r) return;
      r.row.classList.toggle('selected', idx === app.selectedBot);
      r.row.classList.toggle('dead', !b.alive);
      r.subEl.textContent = `K:${b.kills} D:${b.dmg}`;
      r.hpFill.style.width = `${Math.max(0, b.hp)}%`;
      r.hpFill.style.background = b.hp > 50 ? '#34d399' : (b.hp > 25 ? '#f59e0b' : '#f43f5e');
      r.hpVal.textContent = b.hp;
      r.enFill.style.width = `${Math.max(0, b.energy)}%`;
      r.enVal.textContent = b.energy;
      r.gasEl.textContent = `${b.gas}g`;

      if (!b.alive) {
        r.vmPill.textContent = 'DEAD';
        r.vmPill.className = 'vm-pill vm-dead';
      } else {
        r.vmPill.textContent = b.vm_status;
        r.vmPill.className = 'vm-pill ' + (b.vm_status === 'OK' ? 'vm-ok' : (b.vm_status === 'OOG' ? 'vm-oog' : 'vm-trap'));
      }
    });

    if (forceEvents || app.eventCacheLen !== app.cursor) {
      app.eventCacheLen = app.cursor;
      const evWrap = $('eventLog');
      const items = [];
      for (let i = 0; i <= app.cursor; i++) {
        for (const ev of m.frames[i].events) {
          if (ev.kind !== 'dash') items.push(ev);
        }
      }
      const tail = items.slice(-60).reverse();
      evWrap.innerHTML = tail.length
        ? tail.map(ev => `<div class="ev-row ev-${ev.kind}">${escapeHtml(ev.text)}</div>`).join('')
        : '<div class="muted" style="padding:4px;">No events up to this tick.</div>';
    }

    renderCanvas();
  }

  function wrapAngleDiff(a, b) {
    let d = a - b;
    while (d > Math.PI) d -= Math.PI * 2;
    while (d < -Math.PI) d += Math.PI * 2;
    return d;
  }

  function renderCanvas() {
    const m = app.match;
    if (!m || !m.frames.length) return;
    const arena = m.arena;
    const maxIdx = m.frames.length - 1;
    const idx0 = Math.min(maxIdx, Math.floor(app.playFloat));
    const idx1 = Math.min(maxIdx, idx0 + 1);
    const alpha = app.playing ? (app.playFloat - idx0) : 0;

    const f0 = m.frames[idx0];
    const f1 = m.frames[idx1];
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.width;
    const h = canvas.height;
    const padX = 28 * dpr;
    const padY = 24 * dpr;

    // Preserve exact 1:1 world aspect ratio centered in the viewport
    const availW = Math.max(100, w - padX * 2);
    const availH = Math.max(80, h - padY * 2);
    const scale = Math.min(availW / arena.width, availH / arena.height);
    const drawW = arena.width * scale;
    const drawH = arena.height * scale;
    const ox = (w - drawW) / 2;
    const oy = (h - drawH) / 2;

    const sx = wx => ox + wx * scale;
    const sy = wy => oy + (arena.height - wy) * scale;

    ctx.clearRect(0, 0, w, h);

    // Arena surface
    ctx.fillStyle = '#0c0c0f';
    ctx.fillRect(ox, oy, drawW, drawH);

    // CAD grid every 10 units
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.045)';
    ctx.lineWidth = Math.max(1, dpr * 0.75);
    ctx.beginPath();
    for (let gx = 10; gx < arena.width; gx += 10) {
      const px = sx(gx);
      ctx.moveTo(px, oy);
      ctx.lineTo(px, oy + drawH);
    }
    for (let gy = 10; gy < arena.height; gy += 10) {
      const py = sy(gy);
      ctx.moveTo(ox, py);
      ctx.lineTo(ox + drawW, py);
    }
    ctx.stroke();

    // Subtle axis coordinate labels
    ctx.fillStyle = '#52525e';
    ctx.font = `${Math.round(9.5 * dpr)}px ${getMonoFont()}`;
    ctx.textAlign = 'center';
    for (let gx = 0; gx <= arena.width; gx += 20) {
      ctx.fillText(String(gx), sx(gx), oy + drawH + 12 * dpr);
    }
    ctx.textAlign = 'right';
    for (let gy = 0; gy <= arena.height; gy += 20) {
      ctx.fillText(String(gy), ox - 6 * dpr, sy(gy) + 3 * dpr);
    }

    // Arena boundary frame
    ctx.strokeStyle = '#2d2d37';
    ctx.lineWidth = 1.5 * dpr;
    ctx.strokeRect(ox, oy, drawW, drawH);

    // Storm exterior overlay (evenodd) + crisp dashed ring
    const zr = f0.zr + (f1.zr - f0.zr) * alpha;
    const zrPx = zr * scale;
    const cx = sx(arena.width / 2);
    const cy = sy(arena.height / 2);
    ctx.save();
    ctx.beginPath();
    ctx.rect(ox, oy, drawW, drawH);
    ctx.moveTo(cx + zrPx, cy);
    ctx.arc(cx, cy, zrPx, 0, Math.PI * 2, true);
    ctx.fillStyle = 'rgba(168, 85, 247, 0.055)';
    ctx.fill('evenodd');

    ctx.beginPath();
    ctx.arc(cx, cy, zrPx, 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(192, 132, 252, 0.72)';
    ctx.lineWidth = 1.25 * dpr;
    ctx.setLineDash([5 * dpr, 4 * dpr]);
    ctx.stroke();
    ctx.restore();

    // Architectural hatched cover pillars
    arena.pillars.forEach(p => {
      const px = sx(p.x);
      const py = sy(p.y + p.h);
      const pw = p.w * scale;
      const ph = p.h * scale;
      ctx.save();
      ctx.fillStyle = '#141418';
      ctx.fillRect(px, py, pw, ph);
      ctx.beginPath();
      ctx.rect(px, py, pw, ph);
      ctx.clip();
      ctx.strokeStyle = '#24242d';
      ctx.lineWidth = 1 * dpr;
      ctx.beginPath();
      for (let d = -ph; d < pw + ph; d += 7 * dpr) {
        ctx.moveTo(px + d, py);
        ctx.lineTo(px + d - ph, py + ph);
      }
      ctx.stroke();
      ctx.restore();
      ctx.strokeStyle = '#3b3b47';
      ctx.lineWidth = 1 * dpr;
      ctx.strokeRect(px, py, pw, ph);
    });

    // Hit shockwaves & damage ticks from nearby ticks
    for (let back = 0; back < 4; back++) {
      const hi = idx0 - back;
      if (hi < 0) break;
      const frac = (back + alpha) / 4;
      for (const ev of m.frames[hi].events) {
        if (ev.kind === 'hit' && ev.x !== undefined) {
          const hx = sx(ev.x);
          const hy = sy(ev.y);
          const r = (3 + frac * 14) * dpr;
          ctx.beginPath();
          ctx.arc(hx, hy, r, 0, Math.PI * 2);
          ctx.strokeStyle = `rgba(236, 236, 240, ${(1 - frac) * 0.75})`;
          ctx.lineWidth = 1.25 * dpr;
          ctx.stroke();
        }
      }
    }

    // Projectiles (clean vector tracers)
    for (const p of f0.projectiles) {
      const [owner, px0, py0, vx, vy] = p;
      const px = px0 + vx * alpha;
      const py = py0 + vy * alpha;
      const col = (m.bot_summary[owner] && m.bot_summary[owner].color) || '#ececf0';
      ctx.beginPath();
      ctx.moveTo(sx(px - vx * 0.9), sy(py - vy * 0.9));
      ctx.lineTo(sx(px), sy(py));
      ctx.strokeStyle = col;
      ctx.lineWidth = 2 * dpr;
      ctx.lineCap = 'round';
      ctx.stroke();
    }

    // Bots
    const brPx = arena.bot_radius * scale;
    f0.bots.forEach((b0, idx) => {
      const meta = m.bot_summary[idx];
      const b1 = f1.bots[idx] || b0;
      if (!b0.alive) {
        const bx = sx(b0.x);
        const by = sy(b0.y);
        const arm = brPx * 0.52;
        ctx.beginPath();
        ctx.moveTo(bx - arm, by - arm);
        ctx.lineTo(bx + arm, by + arm);
        ctx.moveTo(bx + arm, by - arm);
        ctx.lineTo(bx - arm, by + arm);
        ctx.strokeStyle = '#3f3f4a';
        ctx.lineWidth = 1.5 * dpr;
        ctx.stroke();
        return;
      }

      const bxWorld = b0.x + (b1.x - b0.x) * alpha;
      const byWorld = b0.y + (b1.y - b0.y) * alpha;
      const heading = b0.heading + wrapAngleDiff(b1.heading, b0.heading) * alpha;
      const bx = sx(bxWorld);
      const by = sy(byWorld);

      // Subtle wake path (last 6 frames)
      ctx.beginPath();
      let started = false;
      for (let t = Math.max(0, idx0 - 6); t <= idx0; t++) {
        const tb = m.frames[t].bots[idx];
        if (!tb || !tb.alive) continue;
        if (!started) {
          ctx.moveTo(sx(tb.x), sy(tb.y));
          started = true;
        } else {
          ctx.lineTo(sx(tb.x), sy(tb.y));
        }
      }
      if (started) {
        ctx.lineTo(bx, by);
        ctx.strokeStyle = meta.color + '33';
        ctx.lineWidth = 1.5 * dpr;
        ctx.stroke();
      }

      // Shield perimeter
      if (b0.shield) {
        ctx.beginPath();
        ctx.arc(bx, by, brPx + 4 * dpr, 0, Math.PI * 2);
        ctx.fillStyle = 'rgba(56, 189, 248, 0.1)';
        ctx.fill();
        ctx.strokeStyle = '#38bdf8';
        ctx.lineWidth = 1.2 * dpr;
        ctx.stroke();
      }

      // Selection corner brackets
      if (idx === app.selectedBot) {
        const pad = brPx + 7 * dpr;
        const leg = 5 * dpr;
        ctx.strokeStyle = '#ececf0';
        ctx.lineWidth = 1.25 * dpr;
        ctx.beginPath();
        ctx.moveTo(bx - pad, by - pad + leg); ctx.lineTo(bx - pad, by - pad); ctx.lineTo(bx - pad + leg, by - pad);
        ctx.moveTo(bx + pad - leg, by - pad); ctx.lineTo(bx + pad, by - pad); ctx.lineTo(bx + pad, by - pad + leg);
        ctx.moveTo(bx - pad, by + pad - leg); ctx.lineTo(bx - pad, by + pad); ctx.lineTo(bx - pad + leg, by + pad);
        ctx.moveTo(bx + pad - leg, by + pad); ctx.lineTo(bx + pad, by + pad); ctx.lineTo(bx + pad, by + pad - leg);
        ctx.stroke();
      }

      // Chassis body
      ctx.beginPath();
      ctx.arc(bx, by, brPx, 0, Math.PI * 2);
      ctx.fillStyle = '#09090b';
      ctx.fill();
      ctx.strokeStyle = meta.color;
      ctx.lineWidth = 2.2 * dpr;
      ctx.stroke();

      // Inner core dot
      ctx.beginPath();
      ctx.arc(bx, by, brPx * 0.34, 0, Math.PI * 2);
      ctx.fillStyle = meta.color;
      ctx.fill();

      // Cannon barrel
      const tx = bx + Math.cos(heading) * (brPx * 1.45);
      const ty = by - Math.sin(heading) * (brPx * 1.45);
      ctx.beginPath();
      ctx.moveTo(bx, by);
      ctx.lineTo(tx, ty);
      ctx.strokeStyle = '#ececf0';
      ctx.lineWidth = 2 * dpr;
      ctx.stroke();

      // Compact tag below chassis
      ctx.font = `600 ${Math.round(10 * dpr)}px ${getMonoFont()}`;
      ctx.textAlign = 'center';
      ctx.fillStyle = '#d4d4dc';
      ctx.fillText(`${meta.name} ${b0.hp}`, bx, by + brPx + 12 * dpr);
    });
  }

  function getMonoFont() {
    return 'ui-monospace, "Cascadia Code", Consolas, monospace';
  }

  canvas.addEventListener('click', e => {
    const m = app.match;
    if (!m) return;
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const cx = (e.clientX - rect.left) * dpr;
    const cy = (e.clientY - rect.top) * dpr;
    const padX = 28 * dpr, padY = 24 * dpr;
    const availW = Math.max(100, canvas.width - padX * 2);
    const availH = Math.max(80, canvas.height - padY * 2);
    const scale = Math.min(availW / m.arena.width, availH / m.arena.height);
    const ox = (canvas.width - m.arena.width * scale) / 2;
    const oy = (canvas.height - m.arena.height * scale) / 2;

    const f = m.frames[app.cursor];
    let best = -1, bestD = Infinity;
    f.bots.forEach((b, idx) => {
      if (!b.alive) return;
      const bx = ox + b.x * scale;
      const by = oy + (m.arena.height - b.y) * scale;
      const d = Math.hypot(bx - cx, by - cy);
      if (d < 34 * dpr && d < bestD) { bestD = d; best = idx; }
    });
    if (best >= 0) {
      app.selectedBot = best;
      updateArenaUI(false);
    }
  });

  function tickLoop(ts) {
    if (!app.lastFrameTs) app.lastFrameTs = ts;
    const dt = Math.min(0.08, (ts - app.lastFrameTs) / 1000);
    app.lastFrameTs = ts;

    if (app.playing && app.match && app.match.frames.length > 1) {
      const maxIdx = app.match.frames.length - 1;
      if (app.playFloat < maxIdx) {
        app.playFloat = Math.min(maxIdx, app.playFloat + dt * 30 * app.speed);
        const nextCursor = Math.floor(app.playFloat);
        if (nextCursor !== app.cursor) {
          app.cursor = nextCursor;
          updateArenaUI(false);
        } else {
          renderCanvas();
        }
        if (app.playFloat >= maxIdx) {
          app.cursor = maxIdx;
          app.playing = false;
          syncPlayButton();
          updateArenaUI(false);
        }
      }
    }
    requestAnimationFrame(tickLoop);
  }

  function syncPlayButton() {
    $('iconPlay').style.display = app.playing ? 'none' : 'block';
    $('iconPause').style.display = app.playing ? 'block' : 'none';
  }

  function togglePlay() {
    if (!app.match) return;
    const maxIdx = app.match.frames.length - 1;
    if (app.cursor >= maxIdx) {
      app.cursor = 0;
      app.playFloat = 0;
    }
    app.playing = !app.playing;
    syncPlayButton();
    updateArenaUI(false);
  }

  function stepTick(delta) {
    if (!app.match) return;
    app.playing = false;
    syncPlayButton();
    const maxIdx = app.match.frames.length - 1;
    app.cursor = Math.max(0, Math.min(maxIdx, app.cursor + delta));
    app.playFloat = app.cursor;
    updateArenaUI(false);
  }

  $('btnPlayPause').onclick = togglePlay;
  $('btnStepBack').onclick = () => stepTick(-1);
  $('btnStepFwd').onclick = () => stepTick(1);
  $('btnReplayStart').onclick = () => {
    app.playing = false;
    syncPlayButton();
    app.cursor = 0;
    app.playFloat = 0;
    updateArenaUI(false);
  };

  $('speedGroup').querySelectorAll('.seg-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      $('speedGroup').querySelectorAll('.seg-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      app.speed = Number(btn.dataset.speed);
    });
  });

  // Pointer-draggable timeline scrubber
  const track = $('timelineTrack');
  let scrubbing = false;
  function seekFromPointer(clientX) {
    if (!app.match || !app.match.frames.length) return;
    const rect = track.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
    const maxIdx = app.match.frames.length - 1;
    app.playing = false;
    syncPlayButton();
    app.cursor = Math.round(frac * maxIdx);
    app.playFloat = app.cursor;
    updateArenaUI(false);
  }
  track.addEventListener('pointerdown', e => {
    scrubbing = true;
    track.setPointerCapture(e.pointerId);
    seekFromPointer(e.clientX);
  });
  track.addEventListener('pointermove', e => {
    if (scrubbing) seekFromPointer(e.clientX);
  });
  track.addEventListener('pointerup', e => {
    scrubbing = false;
    if (track.hasPointerCapture(e.pointerId)) track.releasePointerCapture(e.pointerId);
  });

  $('btnRunSeed').onclick = () => runMatch(Number($('seedInput').value) || 42);
  $('btnRandomSeed').onclick = () => runMatch(Math.floor(Math.random() * 100000));

  $('btnCopyHash').onclick = async () => {
    const val = $('hashValue').textContent;
    try {
      await navigator.clipboard.writeText(val);
      $('verifyReadout').textContent = 'copied';
      setTimeout(() => {
        if (app.match) {
          $('verifyReadout').textContent = `${app.match.frames.length - 1} ticks verified (${(app.match.replay_bytes / 1024).toFixed(1)} KB .phxr)`;
        }
      }, 1100);
    } catch (_) {}
  };

  // VM Inspector
  $('btnCaptureTrace').onclick = async () => {
    if (!app.match) return;
    app.playing = false;
    syncPlayButton();
    const tick = app.match.frames[app.cursor].t;
    const res = await api('/api/trace-tick', {
      seed: app.match.seed,
      tick,
      bot_idx: app.selectedBot,
      max_steps: 350,
      bots: app.rosterBots,
    });
    if (!res.ok) {
      $('dbgStepMeta').textContent = res.error;
      return;
    }
    app.dbgTrace = res;
    app.dbgStep = 0;
    renderDebugger();
  };
  $('btnDbgPrev').onclick = () => {
    if (!app.dbgTrace) return;
    app.dbgStep = Math.max(0, app.dbgStep - 1);
    renderDebugger();
  };
  $('btnDbgNext').onclick = () => {
    if (!app.dbgTrace) return;
    app.dbgStep = Math.min(app.dbgTrace.steps.length - 1, app.dbgStep + 1);
    renderDebugger();
  };
  $('dbgStepRange').oninput = e => {
    if (!app.dbgTrace) return;
    app.dbgStep = Number(e.target.value);
    renderDebugger();
  };

  function renderDebugger() {
    const tr = app.dbgTrace;
    if (!tr || !tr.steps.length) return;
    const step = tr.steps[app.dbgStep];
    const prev = app.dbgStep > 0 ? tr.steps[app.dbgStep - 1] : null;
    $('dbgTargetLabel').textContent = `${tr.bot_name} @ T${tr.tick}`;
    $('btnDbgPrev').disabled = app.dbgStep === 0;
    $('btnDbgNext').disabled = app.dbgStep >= tr.steps.length - 1;
    const range = $('dbgStepRange');
    range.disabled = false;
    range.max = tr.steps.length - 1;
    range.value = app.dbgStep;

    $('dbgStepMeta').textContent =
      `step ${app.dbgStep + 1}/${tr.steps.length} · pc=${step.pc} · ${step.status} · depth=${step.depth} · gas=${step.gas_used}/${step.gas_used + step.gas_rem}`;

    const asmEl = $('dbgDisasm');
    const winStart = Math.max(0, step.pc - 4);
    const winEnd = Math.min(tr.disasm_lines.length, winStart + 12);
    asmEl.innerHTML = tr.disasm_lines.slice(winStart, winEnd).map(l => {
      const on = l.pc === step.pc;
      const mark = on ? '>' : ' ';
      return `<div class="asm-row${on ? ' active' : ''}">${mark} ${String(l.pc).padStart(4, '0')}  ${escapeHtml(l.text)}</div>`;
    }).join('');

    $('dbgRegs').innerHTML = step.regs.map((v, i) => {
      const chg = prev && prev.regs[i] !== v;
      const fixStr = (v / 65536).toFixed(4);
      return `<div class="reg-box${chg ? ' changed' : ''}" title="r${i}: i32=${v}, fix=${fixStr}">r${i}:${v}</div>`;
    }).join('');

    $('dbgState').textContent = step.state.length
      ? 'state: ' + step.state.map((v, i) => `${tr.state_names[i] || ('s' + i)}=${v}`).join('  ')
      : 'state: (none)';
  }

  // ==================== EDITOR ====================
  function buildFileTabs() {
    const wrap = $('fileTabs');
    wrap.innerHTML = '';
    app.templates.forEach((tpl, idx) => {
      const btn = document.createElement('button');
      btn.className = 'file-tab' + (idx === app.activeFileIdx ? ' active' : '');
      btn.textContent = `${tpl.name}.phx`;
      btn.onclick = () => loadTemplate(idx);
      wrap.appendChild(btn);
    });
  }

  function loadTemplate(idx) {
    app.activeFileIdx = idx;
    document.querySelectorAll('.file-tab').forEach((b, i) => b.classList.toggle('active', i === idx));
    const tpl = app.templates[idx];
    if (!tpl) return;
    $('editorBotName').value = tpl.name;
    $('codeInput').value = tpl.source;
    updateGutter();
    updateCursorPos();
    compileEditor();
  }

  const codeInput = $('codeInput');
  const gutter = $('lineGutter');

  function updateGutter() {
    const count = codeInput.value.split('\n').length;
    let html = '';
    for (let i = 1; i <= count; i++) {
      const errCls = app.errorLines.has(i) ? ' err' : '';
      html += `<div class="gutter-ln${errCls}">${i}</div>`;
    }
    gutter.innerHTML = html;
  }

  function syncGutterScroll() {
    gutter.scrollTop = codeInput.scrollTop;
  }

  function updateCursorPos() {
    const upTo = codeInput.value.slice(0, codeInput.selectionStart);
    const lines = upTo.split('\n');
    $('cursorPos').textContent = `Ln ${lines.length}, Col ${lines[lines.length - 1].length + 1}`;
  }

  codeInput.addEventListener('input', () => { updateGutter(); updateCursorPos(); });
  codeInput.addEventListener('scroll', syncGutterScroll);
  codeInput.addEventListener('click', updateCursorPos);
  codeInput.addEventListener('keyup', updateCursorPos);
  codeInput.addEventListener('keydown', e => {
    if (e.key === 'Tab') {
      e.preventDefault();
      const s = codeInput.selectionStart, end = codeInput.selectionEnd;
      codeInput.value = codeInput.value.slice(0, s) + '    ' + codeInput.value.slice(end);
      codeInput.selectionStart = codeInput.selectionEnd = s + 4;
      updateGutter();
      updateCursorPos();
    } else if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      compileEditor();
    }
  });

  async function compileEditor() {
    const name = $('editorBotName').value.trim() || 'custom_bot';
    const source = codeInput.value;
    const opt = $('optToggle').checked;
    const res = await api('/api/compile', { name, source, opt });

    app.errorLines.clear();
    if (res.ok) {
      updateGutter();
      const r = res.report, o = res.opt_stats;
      $('verifierStatus').textContent = 'VERIFIED SAFE';
      $('verifierStatus').className = 'verify-ok';
      $('verifierMeta').textContent = `${r.fingerprint} · ${res.bytecode_bytes} B .phxb (CRC32C)`;
      $('verifierDetails').innerHTML = `
        <div class="v-metrics">
          <div class="v-cell"><div class="v-k">Insns</div><div class="v-v">${r.instructions}</div></div>
          <div class="v-cell"><div class="v-k">Blocks</div><div class="v-v">${r.basic_blocks}</div></div>
          <div class="v-cell"><div class="v-k">Loops</div><div class="v-v">${r.back_edges}</div></div>
          <div class="v-cell"><div class="v-k">Depth</div><div class="v-v">${r.max_call_depth}/16</div></div>
          <div class="v-cell"><div class="v-k">Slots</div><div class="v-v">${r.state_slots}/64</div></div>
          <div class="v-cell"><div class="v-k">Arrays</div><div class="v-v">${r.state_arrays}/16</div></div>
        </div>
        <div class="opt-line">opt(${opt ? '-O1' : '-O0'}): folded=${o.consts_folded} · alg=${o.algebraic_simplifications} · prop=${o.copies_propagated} · dce=${o.dead_insts_removed} · cfg_pruned=${o.dead_blocks_removed}</div>`;

      renderDisasmTable(res.disasm);
      $('irView').textContent = res.ir_dump;
      return true;
    }

    res.diagnostics.forEach(d => app.errorLines.add(d.line));
    updateGutter();
    $('verifierStatus').textContent = `${res.diagnostics.length} COMPILE ERROR(S)`;
    $('verifierStatus').className = 'verify-err';
    $('verifierMeta').textContent = 'click diagnostic to jump to line';
    const wrap = $('verifierDetails');
    wrap.innerHTML = '';
    res.diagnostics.forEach(d => {
      const div = document.createElement('div');
      div.className = 'diag-item';
      div.textContent = d.rendered;
      div.onclick = () => jumpToLine(d.line);
      wrap.appendChild(div);
    });
    return false;
  }

  function jumpToLine(lineNum) {
    const lines = codeInput.value.split('\n');
    let offset = 0;
    for (let i = 0; i < Math.min(lines.length, lineNum - 1); i++) {
      offset += lines[i].length + 1;
    }
    codeInput.focus();
    codeInput.setSelectionRange(offset, offset);
    updateCursorPos();
  }

  function renderDisasmTable(text) {
    const wrap = $('disasmView');
    const html = text.split('\n').map(line => {
      if (!line.trim()) return '';
      if (line.startsWith(';')) {
        return `<div class="d-row"><span class="d-comment" style="grid-column:1 / -1;">${escapeHtml(line)}</span></div>`;
      }
      if (line.startsWith('fn ')) {
        return `<div class="d-fn">${escapeHtml(line)}</div>`;
      }
      const m = line.match(/^\s*(\d+):\s+(L\d+)?\s+(\S+)\s*(.*)$/);
      if (m) {
        const [, pc, ln, op, rest] = m;
        const [args, cmt] = rest.split(';');
        const cmtHtml = cmt !== undefined ? ` <span class="d-comment">;${escapeHtml(cmt)}</span>` : '';
        return `<div class="d-row"><span class="d-pc">${pc}</span><span class="d-ln">${ln || ''}</span><span class="d-op">${escapeHtml(op)}</span><span class="d-args">${escapeHtml((args || '').trimEnd())}${cmtHtml}</span></div>`;
      }
      return `<div class="d-row"><span style="grid-column:1 / -1;">${escapeHtml(line)}</span></div>`;
    }).join('');
    wrap.innerHTML = html;
  }

  function renderSyscallTable(filter) {
    const q = filter.trim().toLowerCase();
    const rows = SYSCALL_REF.filter(([name, sig, , desc]) =>
      !q || name.toLowerCase().includes(q) || sig.toLowerCase().includes(q) || desc.toLowerCase().includes(q)
    );
    $('syscallTableWrap').innerHTML = `
      <table class="data-table">
        <thead><tr><th>Syscall</th><th>Signature</th><th>Gas</th><th>Description</th></tr></thead>
        <tbody>
          ${rows.map(([n, s, g, d]) => `<tr><td>${escapeHtml(n)}</td><td class="muted">${escapeHtml(s)}</td><td>${g}</td><td style="font-family:var(--sans);">${escapeHtml(d)}</td></tr>`).join('')}
        </tbody>
      </table>`;
  }

  $('syscallFilter').addEventListener('input', e => renderSyscallTable(e.target.value));
  $('optToggle').addEventListener('change', compileEditor);
  $('btnVerifyCode').onclick = compileEditor;
  $('btnDeployArena').onclick = async () => {
    if (!(await compileEditor())) return;
    const name = $('editorBotName').value.trim() || 'custom_bot';
    const source = codeInput.value;
    const idx = app.rosterBots.findIndex(b => b.name.toLowerCase() === name.toLowerCase());
    if (idx >= 0) {
      app.rosterBots[idx] = { name, source };
    } else if (app.rosterBots.length >= 6) {
      app.rosterBots[app.rosterBots.length - 1] = { name, source };
    } else {
      app.rosterBots.push({ name, source });
    }
    switchTab('arena');
    await runMatch(Number($('seedInput').value) || 42);
  };

  // ==================== BENCHMARKS ====================
  $('tourneyFormatGroup').querySelectorAll('.seg-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      $('tourneyFormatGroup').querySelectorAll('.seg-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      app.tourneyFormat = btn.dataset.format;
    });
  });

  $('btnRunTourney').onclick = async () => {
    const btn = $('btnRunTourney');
    btn.disabled = true;
    btn.textContent = 'Running...';
    const res = await api('/api/tourney', {
      format: app.tourneyFormat,
      rounds: Number($('tourneyRounds').value),
      seed: Number($('tourneySeed').value) || 1337,
      audit_determinism: $('tourneyAudit').checked,
      bots: app.rosterBots,
    });
    btn.disabled = false;
    btn.textContent = 'Run Benchmark';
    if (!res.ok) return;

    const r = res.report;
    app.runCount += 1;
    $('tMps').textContent = `${r.matches_per_sec.toFixed(1)} matches/s`;
    $('tMips').textContent = `${r.vm_mips.toFixed(1)} VM MIPS`;
    $('tVolume').textContent = `${r.total_ticks.toLocaleString()} ticks / ${r.threads_used}T`;
    $('tAudit').textContent = r.audit
      ? (r.audit.passed ? `PASS (${r.audit.hashes_matched}/${r.audit.matches_audited})` : 'FAIL')
      : 'Skipped';
    $('tAudit').style.color = r.audit && r.audit.passed ? 'var(--green)' : 'var(--text-0)';

    const minElo = Math.min(...r.standings.map(s => s.elo), 1350);
    const maxElo = Math.max(...r.standings.map(s => s.elo), 1650);
    $('eloBody').innerHTML = r.standings.map((s, idx) => {
      const pct = Math.max(8, Math.min(100, ((s.elo - minElo + 30) / (maxElo - minElo + 60)) * 100));
      const tpl = app.templates.find(t => t.name.toLowerCase() === s.name.toLowerCase());
      const col = tpl ? tpl.color : '#38bdf8';
      const winPct = s.matches > 0 ? ((s.wins / s.matches) * 100).toFixed(1) : '0.0';
      return `<tr>
        <td>#${idx + 1}</td>
        <td style="font-family:var(--sans);font-weight:600;"><span class="bot-dot" style="background:${col};"></span>${escapeHtml(s.name)}</td>
        <td>${s.elo.toFixed(1)} <span class="elo-bar"><div class="elo-fill" style="width:${pct}%"></div></span></td>
        <td>${s.wins}-${s.losses}-${s.draws}</td>
        <td>${winPct}%</td>
        <td>${s.kills}</td>
        <td>${s.damage_dealt}</td>
        <td>${s.avg_gas_per_tick.toFixed(1)}</td>
        <td>${s.avg_survival_ticks.toFixed(0)}t</td>
        <td>${s.traps}/${s.out_of_gas}</td>
      </tr>`;
    }).join('');

    const hist = $('tourneyHistory');
    if (app.runCount === 1) hist.innerHTML = '';
    const leader = r.standings[0];
    const row = document.createElement('div');
    row.className = 'hist-row';
    row.innerHTML = `<span>#${app.runCount} ${r.format_name} (${r.matches_played} matches, ${r.threads_used}T) &middot; top: ${escapeHtml(leader.name)} (${leader.elo.toFixed(1)})</span><span class="muted">${r.matches_per_sec.toFixed(0)} m/s &middot; ${r.vm_mips.toFixed(1)} MIPS</span>`;
    hist.prepend(row);
  };

  // Shortcuts & Modal
  function toggleModal(open) {
    $('shortcutsModal').hidden = !open;
  }
  $('btnShortcuts').onclick = () => toggleModal(true);
  $('btnCloseModal').onclick = () => toggleModal(false);
  $('shortcutsModal').addEventListener('click', e => {
    if (e.target === $('shortcutsModal')) toggleModal(false);
  });

  window.addEventListener('keydown', e => {
    if (e.key === 'Escape') {
      toggleModal(false);
      return;
    }
    const tag = document.activeElement && document.activeElement.tagName;
    if (tag === 'TEXTAREA' || tag === 'INPUT' || tag === 'SELECT') return;

    if (e.key === '?') { e.preventDefault(); toggleModal($('shortcutsModal').hidden); }
    else if (e.key === '1') switchTab('arena');
    else if (e.key === '2') switchTab('editor');
    else if (e.key === '3') switchTab('benchmarks');
    else if (e.key === ' ') { e.preventDefault(); togglePlay(); }
    else if (e.key === 'ArrowLeft') { e.preventDefault(); stepTick(-1); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); stepTick(1); }
    else if (e.key === 'Home') { e.preventDefault(); $('btnReplayStart').click(); }
    else if (e.key === 'End' && app.match) {
      e.preventDefault();
      stepTick(app.match.frames.length - 1);
    }
  });

  function escapeHtml(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  boot();
})();
