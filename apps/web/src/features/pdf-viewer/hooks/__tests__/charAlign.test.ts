/**
 * charAlign — 反向定位算法单元测试
 *
 * 覆盖：基本匹配、空白差异、hyphen-newline 断行、太短拒绝、不匹配、
 * 真实 MinerU vs PDFium 文本差异场景。
 */
import { describe, it, expect } from 'vitest';
import { locateTextInPage } from '../charAlign';

describe('locateTextInPage', () => {
  it('完全一致的文本直接定位', () => {
    const page = 'Hello world.';
    const target = 'Hello world.';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: 12 });
  });

  it('忽略 pageText 中的额外空白', () => {
    // PDFium 字符流常常是 "Hello\nworld"（带换行），MinerU 是 "Hello world"
    const page = 'Hello\nworld.';
    const target = 'Hello world.';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: 12 });
  });

  it('hyphen-newline 断行被合并', () => {
    // PDFium 字符流：单词按音节断行，行尾插入 hyphen
    // MinerU 已经把断行合并成完整单词
    const page = 'human epider-\nmal growth factor';
    const target = 'human epidermal growth factor';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: page.length });
  });

  it('大小写不敏感', () => {
    const page = 'Hello WORLD.';
    const target = 'hello world.';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: 12 });
  });

  it('保留数字和标点（两边一致时）', () => {
    const page = 'Value is 3.14!';  // 长度 14
    const target = 'Value is 3.14!';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: 14 });
  });

  it('target 太短（< 6 字符）拒绝匹配', () => {
    const page = 'Hello world.';
    const target = 'Hi';
    const r = locateTextInPage(page, target);
    expect(r).toBeNull();
  });

  it('target 不存在于 pageText 时返回 null', () => {
    const page = 'The quick brown fox.';
    const target = 'jumped over the lazy dog';
    const r = locateTextInPage(page, target);
    expect(r).toBeNull();
  });

  it('返回的是 pageText 原始坐标（含空白的下标）', () => {
    // pageText 前缀有换行：定位结果应反映原始位置
    const page = 'Title\n\nAbstract body text here.';
    const target = 'Abstract body text here.';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    expect(r!.start).toBe(7); // "Title\n\n" 占 7 个字符
    expect(page.slice(r!.start, r!.end).replace(/\s+/g, ' '))
      .toBe('Abstract body text here.');
  });

  it('真实场景：MinerU 段落原文在 PDFium 字符流里定位', () => {
    // 模拟 PDFium 抽出的多行字符流（论文摘要）
    const pageText = [
      'Article',
      'Accurate assessment of human epidermal growth factor',
      'receptor 2 (HER2) status is crucial for effective',
      'breast cancer treatment planning.',
      '',
      'Breast cancer is the most common cancer worldwide.',
    ].join('\n');

    // MinerU 提取的同一句（已合并换行）
    const mineruSentence = 'Accurate assessment of human epidermal growth factor receptor 2 (HER2) status is crucial for effective breast cancer treatment planning.';

    const r = locateTextInPage(pageText, mineruSentence);
    expect(r).not.toBeNull();
    expect(r!.start).toBe(8); // "Article\n" 之后
    // end 应覆盖到 "planning." 的句点
    expect(pageText.slice(r!.start, r!.end).replace(/\s+/g, ' '))
      .toBe(mineruSentence);
  });

  it('多页字符流中只匹配第一次出现', () => {
    const page = 'Repeat. Repeat. Repeat.';
    const target = 'Repeat.';
    const r = locateTextInPage(page, target);
    expect(r).toEqual({ start: 0, end: 7 });
  });

  // ===== Anchor fallback 场景 =====

  it('target 中段有字符差异时，前后 anchor 兜底定位', () => {
    // pageText 用半角括号 "(HER2)"，target 用全角 "（HER2）"
    // 精确匹配失败，但前 15 + 后 15 字符 anchor 能匹配
    const page = 'Accurate assessment of human epidermal (HER2) status is crucial outcomes';
    const target = 'Accurate assessment of human epidermal（HER2）status is crucial outcomes';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    expect(r!.start).toBe(0);
    expect(r!.end).toBe(page.length);
  });

  it('target 中段缺失字符时，anchor 兜底', () => {
    // pageText 有 "extra junk"，target 没有 → 精确失败，anchor 兜底
    const page = 'The quick brown EXTRA JUNK fox jumps over';
    const target = 'The quick brown fox jumps over';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    expect(r!.start).toBe(0);
    expect(r!.end).toBe(page.length);
  });

  it('target 首 15 字符在 pageText 中不存在时返回 null', () => {
    const page = 'Different starting words here altogether';
    const target = 'Completely different content than page';
    const r = locateTextInPage(page, target);
    expect(r).toBeNull();
  });

  it('target 尾 15 字符在 pageText 中不存在时返回 null', () => {
    // prefix 能匹配，但 suffix 不能
    const page = 'Shared prefix content that goes nowhere meaningful';
    const target = 'Shared prefix content that ends with nonexistent suffix';
    const r = locateTextInPage(page, target);
    expect(r).toBeNull();
  });

  it('anchor 兜底时，反映射回的位置覆盖原 pageText 的实际范围', () => {
    // pageText 中间有空白，归一化后无空白；anchor 定位后反映射应给原坐标
    const page = 'Start\n\nAccurate assessment of human\nstatus crucial\noutcomes';
    const target = 'Accurate assessment of human status crucial outcomes';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    // 反映射的 start/end 应在原 pageText 坐标空间
    expect(r!.start).toBeGreaterThanOrEqual(0);
    expect(r!.end).toBeLessThanOrEqual(page.length);
    expect(page.slice(r!.start, r!.end).replace(/\s+/g, ' '))
      .toContain('Accurate');
  });

  // ===== PDF 不可见标记字符场景 =====
  // 这些字符（软连字符 / 零宽空格 / 词连接符 / BOM / non-character）由 PDF 作者工具插入，
  // 在 PDFium 字符流里计入但视觉上零宽。getPageGeometry 的 geo.text 保留它们以与
  // runs[].charStart 索引对齐；MinerU 文本通常不含这些标记。
  // charAlign.normalize 必须跳过它们，否则两端长度不等，索引整体错位。

  it('pageText 含软连字符（U+00AD）时仍能匹配无标记的 target', () => {
    // 模拟 PDFium raw 字符流：在 "Accurate" 中间插了软连字符
    const page = 'Accu\xADrate assessment of human epidermal';
    const target = 'Accurate assessment of human epidermal';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    expect(r!.start).toBe(0);
    expect(r!.end).toBe(page.length);
  });

  it('pageText 含零宽空格（U+200B）时不影响定位', () => {
    const page = 'Hello​world.';
    const target = 'Hello world.';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    expect(page.slice(r!.start, r!.end).replace(/[​\s]/g, ''))
      .toBe('Helloworld.');
  });

  it('pageText 含 BOM（U+FEFF）前缀时不偏移后续定位', () => {
    const page = '﻿Hello world.';
    const target = 'Hello world.';
    const r = locateTextInPage(page, target);
    expect(r).not.toBeNull();
    // start 应该指向 H（即索引 1，跳过 BOM）
    expect(page[r!.start]).toBe('H');
  });

  it('真实场景：pageText 前面有一个软连字符，target 无标记时仍正确对齐', () => {
    // 模拟真实 bug 场景：raw 字符流前面有 1 个标记字符，导致 stripped 文本比 raw 短 1
    // 旧代码（用 getTextSlices stripped 文本）会让 charStart 整体偏移 1
    const rawPageText = '­Currently, HER2 status is primarily determined using specimens obtained from needle biopsies in patients before neoadjuvant treatment.';
    const mineruSentence = 'Currently, HER2 status is primarily determined using specimens obtained from needle biopsies in patients before neoadjuvant treatment.';
    const r = locateTextInPage(rawPageText, mineruSentence);
    expect(r).not.toBeNull();
    // start 必须是 1（C 的位置），不是 0（软连字符的位置）
    expect(rawPageText[r!.start]).toBe('C');
  });
});

