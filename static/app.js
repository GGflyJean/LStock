(function () {
  function tickClock() {
    const clock = document.getElementById('utc-clock');
    if (!clock) return;
    const now = new Date();
    clock.textContent = now.toISOString().slice(11, 19) + ' UTC';
  }
  tickClock();
  setInterval(tickClock, 1000);

  const autoRefresh = document.querySelector('[data-auto-refresh]');
  if (autoRefresh) {
    const interval = Number(autoRefresh.dataset.autoRefresh) || 15000;
    setInterval(function () {
      window.location.reload();
    }, interval);
  }

  const button = document.getElementById('refresh-btn');
  if (!button) return;
  const status = document.getElementById('refresh-status');

  button.addEventListener('click', async function () {
    if (button.disabled) return;
    const original = button.textContent;
    button.disabled = true;
    button.textContent = '刷新中…';
    if (status) status.textContent = '正在刷新当前页面…';

    const view = button.dataset.market || 'dual';
    const market = view === 'dual' ? 'both' : view;
    const body = new URLSearchParams({
      csrf: button.dataset.csrf || '',
      market: market,
      tab: button.dataset.tab || 'watch',
      board: button.dataset.board || '',
    });

    try {
      const response = await fetch('/api/refresh', {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: body.toString(),
      });
      const text = await response.text();
      let data = {};
      try {
        data = JSON.parse(text);
      } catch (error) {
        data = {};
      }
      if (!response.ok) {
        if (status) status.textContent = text || '刷新失败，请重试';
        button.disabled = false;
        button.textContent = original;
        return;
      }
      const outcomes = data.outcomes || [];
      const cooldown = outcomes.find(function (item) { return item.status === 'cooldown'; });
      if (cooldown) {
        if (status) status.textContent = '刚刚刷新过，请 ' + cooldown.remaining_secs + ' 秒后再试';
        button.disabled = false;
        button.textContent = original;
        return;
      }
      const failed = outcomes.find(function (item) { return item.status === 'failed'; });
      if (failed) {
        if (status) status.textContent = '刷新失败：' + (failed.error || '未知错误');
        button.disabled = false;
        button.textContent = original;
        return;
      }
      if (status) status.textContent = '刷新完成，正在更新页面…';
      window.location.reload();
    } catch (error) {
      if (status) status.textContent = '刷新失败：' + error;
      button.disabled = false;
      button.textContent = original;
    }
  });
})();

(function () {
  function bucketOf(date, granularity) {
    const parts = String(date).split(/[-/: ]/);
    const year = Number(parts[0]);
    const month = Number(parts[1] || 1);
    if (!Number.isFinite(year)) return null;
    if (granularity === 'year') {
      return { key: String(year), sort: year };
    }
    if (granularity === 'half') {
      const half = month <= 6 ? 1 : 2;
      return { key: year + ' H' + half, sort: year * 10 + half };
    }
    if (granularity === 'quarter') {
      const quarter = Math.ceil(month / 3);
      return { key: year + ' Q' + quarter, sort: year * 10 + quarter };
    }
    return {
      key: year + '-' + String(month).padStart(2, '0'),
      sort: year * 100 + month,
    };
  }

  function aggregate(points, granularity) {
    const buckets = new Map();
    points.forEach(function (point) {
      if (!Number.isFinite(point.value)) return;
      const bucket = bucketOf(point.date, granularity);
      if (!bucket) return;
      if (!buckets.has(bucket.key)) {
        buckets.set(bucket.key, { key: bucket.key, sort: bucket.sort, sum: 0, count: 0 });
      }
      const item = buckets.get(bucket.key);
      item.sum += point.value;
      item.count += 1;
    });
    return Array.from(buckets.values())
      .sort(function (a, b) { return a.sort - b.sort; })
      .map(function (item) {
        return { key: item.key, value: item.sum / item.count };
      });
  }

  function drawMacroChart(canvas, points, unit) {
    const dpr = window.devicePixelRatio || 1;
    const width = Math.max(canvas.clientWidth || 320, 240);
    const height = 180;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    if (!points.length) {
      ctx.fillStyle = '#8b9bb1';
      ctx.font = '13px -apple-system, BlinkMacSystemFont, sans-serif';
      ctx.fillText('暂无历史数据', 12, 28);
      return;
    }

    const padding = { left: 48, right: 12, top: 12, bottom: 26 };
    const plotWidth = width - padding.left - padding.right;
    const plotHeight = height - padding.top - padding.bottom;
    const values = points.map(function (point) { return point.value; });
    let min = Math.min.apply(null, values);
    let max = Math.max.apply(null, values);
    if (min === max) {
      min -= 1;
      max += 1;
    }
    const span = max - min;
    min -= span * 0.08;
    max += span * 0.08;
    const x = function (index) {
      return padding.left + (points.length === 1 ? plotWidth / 2 : plotWidth * index / (points.length - 1));
    };
    const y = function (value) {
      return padding.top + plotHeight * (1 - (value - min) / (max - min));
    };

    ctx.strokeStyle = '#2a3546';
    ctx.lineWidth = 1;
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i <= 4; i++) {
      const value = min + (max - min) * i / 4;
      const py = y(value);
      ctx.beginPath();
      ctx.moveTo(padding.left, py);
      ctx.lineTo(width - padding.right, py);
      ctx.stroke();
      ctx.fillText(value.toFixed(1) + (unit || ''), 4, py + 4);
    }

    if (min < 0 && max > 0) {
      ctx.save();
      ctx.strokeStyle = '#8b9bb1';
      ctx.setLineDash([4, 4]);
      ctx.beginPath();
      ctx.moveTo(padding.left, y(0));
      ctx.lineTo(width - padding.right, y(0));
      ctx.stroke();
      ctx.restore();
    }

    ctx.beginPath();
    points.forEach(function (point, index) {
      if (index === 0) {
        ctx.moveTo(x(index), y(point.value));
      } else {
        ctx.lineTo(x(index), y(point.value));
      }
    });
    ctx.strokeStyle = '#4c8dff';
    ctx.lineWidth = 2;
    ctx.stroke();

    ctx.lineTo(x(points.length - 1), padding.top + plotHeight);
    ctx.lineTo(x(0), padding.top + plotHeight);
    ctx.closePath();
    const gradient = ctx.createLinearGradient(0, padding.top, 0, padding.top + plotHeight);
    gradient.addColorStop(0, 'rgba(76, 141, 255, 0.25)');
    gradient.addColorStop(1, 'rgba(76, 141, 255, 0.02)');
    ctx.fillStyle = gradient;
    ctx.fill();

    const labels = Math.min(6, points.length);
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i < labels; i++) {
      const index = labels === 1 ? 0 : Math.round(i * (points.length - 1) / (labels - 1));
      const label = points[index].key;
      const labelX = x(index);
      ctx.fillText(label, Math.min(Math.max(labelX - 18, padding.left), width - 60), height - 8);
    }
  }

  function renderMacroPane(pane) {
    const select = pane.querySelector('.macro-granularity');
    const granularity = select ? select.value : 'year';
    pane.querySelectorAll('.macro-canvas').forEach(function (canvas) {
      let points = [];
      try {
        points = JSON.parse(canvas.dataset.series || '[]');
      } catch (error) {
        points = [];
      }
      drawMacroChart(canvas, aggregate(points, granularity), canvas.dataset.unit || '');
    });
  }

  function renderAllMacroPanes() {
    document.querySelectorAll('.macro-pane').forEach(renderMacroPane);
  }

  document.querySelectorAll('.macro-pane').forEach(function (pane) {
    const select = pane.querySelector('.macro-granularity');
    if (select) {
      select.addEventListener('change', function () { renderMacroPane(pane); });
    }
  });
  renderAllMacroPanes();

  let resizeTimer = null;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(renderAllMacroPanes, 150);
  });
})();
