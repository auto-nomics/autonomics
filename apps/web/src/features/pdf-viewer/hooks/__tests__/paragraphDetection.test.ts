import { describe, it, expect } from 'vitest';
import { splitIntoSentences } from '../paragraphDetection';

describe('splitIntoSentences', () => {
  // 辅助：只关心切出来的文本，不关心偏移量
  const texts = (s: string): string[] => splitIntoSentences(s).map(r => r.text);

  describe('英文常规句子', () => {
    it('按句号切分', () => {
      expect(texts('Hello world. Foo bar.')).toEqual(['Hello world.', 'Foo bar.']);
    });

    it('按感叹号 / 问号切分', () => {
      expect(texts('Wow! Really? Yes.')).toEqual(['Wow!', 'Really?', 'Yes.']);
    });

    it('保留尾部引号和闭括号', () => {
      expect(texts('He said "hi." Then left.')).toEqual([
        'He said "hi."',
        'Then left.',
      ]);
    });

    it('单句无标点也兜底返回', () => {
      expect(texts('just a phrase')).toEqual(['just a phrase']);
    });
  });

  describe('学术缩写（不应误切）', () => {
    it('e.g. / i.e. / etc.', () => {
      expect(texts('Use tools, e.g. hammers. They work well.')).toEqual([
        'Use tools, e.g. hammers.',
        'They work well.',
      ]);
      expect(texts('This is, i.e., the best. Take it.')).toHaveLength(2);
      expect(texts('Apples, etc. are fruits.')).toHaveLength(1);
    });

    it('Fig. / Eq. / Sec. / Tab. / Vol. / No.', () => {
      expect(texts('See Fig. 2 for details. The result follows.')).toHaveLength(2);
      expect(texts('As shown in Eq. 3, the value converges. Then it stops.')).toHaveLength(2);
      expect(texts('Refer to Sec. 4. The next section continues.')).toHaveLength(2);
    });

    it('Dr. / Prof. / Mr. / Jr. / Sr.', () => {
      expect(texts('Dr. Smith gave a talk. The audience applauded.')).toHaveLength(2);
      expect(texts('Meet Prof. Lee. He is here.')).toHaveLength(2);
    });

    it('et al.（带句点）', () => {
      expect(texts('Smith et al. proposed the method. The community adopted it.')).toEqual([
        'Smith et al. proposed the method.',
        'The community adopted it.',
      ]);
    });

    it('公司后缀 Inc. / Ltd. / Co. / Corp.', () => {
      expect(texts('Acme Inc. released a product. The market reacted.')).toHaveLength(2);
      expect(texts('Foo Ltd. is based in UK. It grows fast.')).toHaveLength(2);
    });

    it('月份 Jan. / Feb. / Mar. 等', () => {
      expect(texts('On Jan. 5th we launched. Sales spiked.')).toHaveLength(2);
    });

    it('页码 p. / pp.', () => {
      expect(texts('See p. 12 for proof. The argument is short.')).toHaveLength(2);
    });
  });

  describe('单字母首字母缩写（J. Smith 模式）', () => {
    it('G. Smith. The ... 不应切成 3 句', () => {
      expect(texts('G. Smith proposed the algorithm. The result is good.')).toEqual([
        'G. Smith proposed the algorithm.',
        'The result is good.',
      ]);
    });

    it('J. K. Rowling 连续首字母缩写', () => {
      expect(texts('J. K. Rowling wrote books. They sold well.')).toHaveLength(2);
    });

    it('句中 Smith, J. (2020). The ... —— J. 后是括号不切，(2020). 后切', () => {
      const out = texts('Smith, J. (2020). The study shows X. It is robust.');
      expect(out).toEqual(['Smith, J. (2020).', 'The study shows X.', 'It is robust.']);
    });
  });

  describe('U.S.A. 连续单字母缩写', () => {
    it('U.S.A. leads 不切（leads 小写）', () => {
      expect(texts('U.S.A. leads the market.')).toEqual(['U.S.A. leads the market.']);
    });

    it('U.S.A. The market ... 应切（最后一个 A. 不是首字母缩写）', () => {
      expect(texts('U.S.A. The market crashed.')).toEqual([
        'U.S.A.',
        'The market crashed.',
      ]);
    });
  });

  describe('数字 + 句点（小数 / 版本号 / 章节号）', () => {
    it('section 3.2. 不应误切', () => {
      expect(texts('See section 3.2. The algorithm converges fast.')).toEqual([
        'See section 3.2.',
        'The algorithm converges fast.',
      ]);
    });

    it('pH 7.4. 不应误切', () => {
      expect(texts('The buffer is pH 7.4. It is stable.')).toEqual([
        'The buffer is pH 7.4.',
        'It is stable.',
      ]);
    });

    it('version 1.0. 不应误切', () => {
      expect(texts('We use version 1.0. The build is green.')).toHaveLength(2);
    });

    it('小数 3.14 不切', () => {
      expect(texts('Pi is 3.14 and e is 2.71. Both are constants.')).toHaveLength(2);
    });
  });

  describe('CJK 句子边界', () => {
    it('中文句号切分', () => {
      expect(texts('今天是周一。明天是周二。')).toEqual([
        '今天是周一。',
        '明天是周二。',
      ]);
    });

    it('中文感叹号 / 问号切分', () => {
      expect(texts('太好了！真的吗？是的。')).toEqual([
        '太好了！',
        '真的吗？',
        '是的。',
      ]);
    });

    it('中英混排', () => {
      expect(texts('如图所示。The result is good. It works.')).toEqual([
        '如图所示。',
        'The result is good.',
        'It works.',
      ]);
    });

    it('分号不切分（按设计）', () => {
      expect(texts('第一部分；第二部分。')).toEqual([
        '第一部分；第二部分。',
      ]);
    });
  });

  describe('图注项 (figure caption items)', () => {
    it('a, ... b, ... c, ... 模式应按项切分', () => {
      expect(texts(
        'a, The first panel shows the result. b, The second panel confirms it. c, The third panel extends the finding.',
      )).toEqual([
        'a, The first panel shows the result.',
        'b, The second panel confirms it.',
        'c, The third panel extends the finding.',
      ]);
    });

    it('正文末尾 + 图注项起始 ("biopsy. c, The...") 应切分', () => {
      // 这是 user 报告的真实 bug：caption b/c/d 因 ". c," 后是小写字母被合并成一句
      expect(texts(
        'biopsy and surgical excision biopsy. c, The large-scale multimodal data involved in this study. d, The workflow of method development, evaluation and analysis.',
      )).toEqual([
        'biopsy and surgical excision biopsy.',
        'c, The large-scale multimodal data involved in this study.',
        'd, The workflow of method development, evaluation and analysis.',
      ]);
    });

    it('列举 a, b, c are... 不应误切（逗号后仍小写）', () => {
      expect(texts('Variables a, b, c are independent.')).toEqual([
        'Variables a, b, c are independent.',
      ]);
    });

    it('图注项后接正文（大写）也正常切', () => {
      // 注：句子末尾避免用单字母（如 "X."）—— 单字母+句点+大写词会触发
      // J. Smith 模式的首字母缩写启发式，这是 splitIntoSentences 的固有边界，与本修复无关
      expect(texts('c, The panel shows the result. The conclusion follows.')).toEqual([
        'c, The panel shows the result.',
        'The conclusion follows.',
      ]);
    });
  });

  describe('偏移量正确性', () => {
    it('start/end 与 text 一致（无 CJK）', () => {
      const ranges = splitIntoSentences('Foo bar. Baz qux.');
      for (const r of ranges) {
        expect(r.text).toBe('Foo bar. Baz qux.'.slice(r.start, r.end).trim());
      }
    });

    it('单句兜底', () => {
      expect(texts('   ')).toEqual([]);
      expect(texts('')).toEqual([]);
    });
  });
});
