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
  const RANGE_YEARS = { '1y': 1, '3y': 3, '5y': 5, '10y': 10, '20y': 20, all: null };

  function parseDate(date) {
    const parts = String(date).split(/[-/: ]/);
    const year = Number(parts[0]);
    const month = Number(parts[1] || 1);
    if (!Number.isFinite(year)) return null;
    return { year, month, sort: year * 100 + month };
  }

  function bucketOf(date, granularity) {
    const parsed = parseDate(date);
    if (!parsed) return null;
    const { year, month } = parsed;
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

  function filterRange(points, range) {
    const years = RANGE_YEARS[range];
    if (years === null || !points.length) return points;
    let latest = -Infinity;
    points.forEach(function (point) {
      const parsed = parseDate(point.date);
      if (parsed) latest = Math.max(latest, parsed.sort);
    });
    if (!Number.isFinite(latest)) return points;
    const latestYear = Math.floor(latest / 100);
    const cutoff = (latestYear - years) * 100 + (latest % 100);
    return points.filter(function (point) {
      const parsed = parseDate(point.date);
      return parsed && parsed.sort >= cutoff;
    });
  }

  function formatValue(value, unit) {
    if (unit === '%') {
      return (value >= 0 ? '+' : '') + value.toFixed(2) + '%';
    }
    return value.toFixed(1) + (unit || '');
  }

  function drawMacroChart(canvas, points, unit, highlightIndex) {
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
      ctx.fillText('暂无数据', 12, 28);
      canvas._macroLayout = { points: [], unit: unit };
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
    const xAt = function (index) {
      return padding.left + (points.length === 1 ? plotWidth / 2 : plotWidth * index / (points.length - 1));
    };
    const yAt = function (value) {
      return padding.top + plotHeight * (1 - (value - min) / (max - min));
    };

    ctx.strokeStyle = '#2a3546';
    ctx.lineWidth = 1;
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i <= 4; i++) {
      const value = min + (max - min) * i / 4;
      const py = yAt(value);
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
      ctx.moveTo(padding.left, yAt(0));
      ctx.lineTo(width - padding.right, yAt(0));
      ctx.stroke();
      ctx.restore();
    }

    ctx.beginPath();
    points.forEach(function (point, index) {
      if (index === 0) {
        ctx.moveTo(xAt(index), yAt(point.value));
      } else {
        ctx.lineTo(xAt(index), yAt(point.value));
      }
    });
    ctx.strokeStyle = '#4c8dff';
    ctx.lineWidth = 2;
    ctx.stroke();

    ctx.lineTo(xAt(points.length - 1), padding.top + plotHeight);
    ctx.lineTo(xAt(0), padding.top + plotHeight);
    ctx.closePath();
    const gradient = ctx.createLinearGradient(0, padding.top, 0, padding.top + plotHeight);
    gradient.addColorStop(0, 'rgba(76, 141, 255, 0.25)');
    gradient.addColorStop(1, 'rgba(76, 141, 255, 0.02)');
    ctx.fillStyle = gradient;
    ctx.fill();

    const radius = points.length > 120 ? 1.2 : 2.2;
    ctx.fillStyle = '#4c8dff';
    points.forEach(function (point, index) {
      ctx.beginPath();
      ctx.arc(xAt(index), yAt(point.value), radius, 0, Math.PI * 2);
      ctx.fill();
    });

    if (highlightIndex >= 0 && highlightIndex < points.length) {
      const point = points[highlightIndex];
      const px = xAt(highlightIndex);
      const py = yAt(point.value);
      ctx.save();
      ctx.strokeStyle = 'rgba(76, 141, 255, 0.45)';
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(px, padding.top);
      ctx.lineTo(px, padding.top + plotHeight);
      ctx.stroke();
      ctx.restore();
      ctx.beginPath();
      ctx.arc(px, py, 4, 0, Math.PI * 2);
      ctx.fillStyle = '#fff';
      ctx.fill();
      ctx.lineWidth = 2;
      ctx.strokeStyle = '#4c8dff';
      ctx.stroke();
    }

    const labels = Math.min(6, points.length);
    ctx.fillStyle = '#8b9bb1';
    ctx.font = '11px -apple-system, BlinkMacSystemFont, sans-serif';
    for (let i = 0; i < labels; i++) {
      const index = labels === 1 ? 0 : Math.round(i * (points.length - 1) / (labels - 1));
      const labelX = xAt(index);
      ctx.fillText(points[index].key, Math.min(Math.max(labelX - 18, padding.left), width - 60), height - 8);
    }

    canvas._macroLayout = {
      points: points,
      unit: unit,
      xAt: xAt,
      yAt: yAt,
      width: width,
      height: height,
    };
  }

  function nearestIndex(canvas, event) {
    const layout = canvas._macroLayout;
    if (!layout || !layout.points.length) return -1;
    const rect = canvas.getBoundingClientRect();
    const mouseX = event.clientX - rect.left;
    let best = -1;
    let bestDistance = Infinity;
    layout.points.forEach(function (point, index) {
      const distance = Math.abs(layout.xAt(index) - mouseX);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = index;
      }
    });
    return bestDistance <= 18 ? best : -1;
  }

  function showTooltip(pane, canvas, index) {
    const layout = canvas._macroLayout;
    const tooltip = pane.querySelector('.macro-tooltip');
    if (!layout || !tooltip || index < 0 || index >= layout.points.length) return;
    const point = layout.points[index];
    const rect = canvas.getBoundingClientRect();
    const left = Math.min(window.innerWidth - 240, rect.left + layout.xAt(index) + 12);
    const top = Math.max(8, rect.top + layout.yAt(point.value) - 34);
    tooltip.textContent = point.key + '：' + formatValue(point.value, layout.unit);
    tooltip.style.left = left + 'px';
    tooltip.style.top = top + 'px';
    tooltip.hidden = false;
  }

  function hideTooltip(pane) {
    const tooltip = pane.querySelector('.macro-tooltip');
    if (tooltip) tooltip.hidden = true;
  }

  function attachCanvasHandlers(canvas, pane) {
    if (canvas.dataset.macroHover === '1') return;
    canvas.dataset.macroHover = '1';
    canvas._macroPinned = -1;

    canvas.addEventListener('mousemove', function (event) {
      const layout = canvas._macroLayout;
      if (!layout || !layout.points.length) return;
      const index = nearestIndex(canvas, event);
      if (index < 0) {
        hideTooltip(pane);
        drawMacroChart(canvas, layout.points, layout.unit, canvas._macroPinned >= 0 ? canvas._macroPinned : -1);
        if (canvas._macroPinned >= 0) showTooltip(pane, canvas, canvas._macroPinned);
        return;
      }
      drawMacroChart(canvas, layout.points, layout.unit, index);
      showTooltip(pane, canvas, index);
    });

    canvas.addEventListener('mouseleave', function () {
      const layout = canvas._macroLayout;
      if (canvas._macroPinned >= 0 && layout) {
        drawMacroChart(canvas, layout.points, layout.unit, canvas._macroPinned);
        showTooltip(pane, canvas, canvas._macroPinned);
      } else {
        hideTooltip(pane);
        if (layout) drawMacroChart(canvas, layout.points, layout.unit, -1);
      }
    });

    canvas.addEventListener('click', function (event) {
      const index = nearestIndex(canvas, event);
      if (index < 0) {
        canvas._macroPinned = -1;
        hideTooltip(pane);
        return;
      }
      canvas._macroPinned = canvas._macroPinned === index ? -1 : index;
      const layout = canvas._macroLayout;
      if (layout) drawMacroChart(canvas, layout.points, layout.unit, canvas._macroPinned);
      if (canvas._macroPinned >= 0) {
        showTooltip(pane, canvas, canvas._macroPinned);
      } else {
        hideTooltip(pane);
      }
    });
  }

  function renderMacroPane(pane) {
    const viewButton = pane.querySelector('.macro-view-button.active');
    const view = viewButton ? viewButton.dataset.view : 'growth';
    const rangeSelect = pane.querySelector('.macro-range');
    const range = rangeSelect ? rangeSelect.value : '20y';
    const granularitySelect = pane.querySelector('.macro-granularity');
    const granularity = granularitySelect ? granularitySelect.value : 'year';

    pane.querySelectorAll('.macro-chart').forEach(function (section) {
      const canvas = section.querySelector('.macro-canvas');
      if (!canvas) return;
      let views = {};
      let series = [];
      try {
        views = JSON.parse(canvas.dataset.views || '{}');
      } catch (error) {
        views = {};
      }
      try {
        series = JSON.parse(canvas.dataset.series || '[]');
      } catch (error) {
        series = [];
      }
      const hasViews = (views.growth && views.growth.length) ||
        (views.yoy && views.yoy.length) ||
        (views.mom && views.mom.length);
      let points = hasViews ? (views[view] || []) : series;
      points = filterRange(points, range);
      const aggregated = aggregate(points, granularity);
      canvas._macroPinned = -1;
      drawMacroChart(canvas, aggregated, canvas.dataset.unit || '', -1);
      attachCanvasHandlers(canvas, pane);

      const valueEl = section.querySelector('.macro-value');
      const noteEl = section.querySelector('.macro-note');
      if (aggregated.length) {
        const last = aggregated[aggregated.length - 1];
        if (valueEl) valueEl.textContent = formatValue(last.value, canvas.dataset.unit || '');
        if (noteEl) noteEl.textContent = last.key;
      } else {
        if (valueEl) valueEl.textContent = '暂无数据';
        if (noteEl) noteEl.textContent = '';
      }
    });
  }

  function renderAllMacroPanes() {
    document.querySelectorAll('.macro-pane').forEach(renderMacroPane);
  }

  document.querySelectorAll('.macro-pane').forEach(function (pane) {
    pane.querySelectorAll('.macro-view-button').forEach(function (button) {
      button.addEventListener('click', function () {
        pane.querySelectorAll('.macro-view-button').forEach(function (item) {
          item.classList.toggle('active', item === button);
        });
        renderMacroPane(pane);
      });
    });
    const rangeSelect = pane.querySelector('.macro-range');
    if (rangeSelect) {
      rangeSelect.addEventListener('change', function () { renderMacroPane(pane); });
    }
    const granularitySelect = pane.querySelector('.macro-granularity');
    if (granularitySelect) {
      granularitySelect.addEventListener('change', function () { renderMacroPane(pane); });
    }
  });
  renderAllMacroPanes();

  let resizeTimer = null;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(renderAllMacroPanes, 150);
  });
})();
