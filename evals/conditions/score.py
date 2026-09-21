#!/usr/bin/env -S uv run --script --quiet
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Network-free checks and review packets; editorial grades are supplied locally."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import statistics
from typing import Any

ROOT = Path(__file__).resolve().parent
PROMPT = (ROOT / 'judge_prompt.txt').read_text()


def quality_pass(grade: dict[str, Any]) -> bool:
    return (grade.get('accurate') is True and grade.get('qualifications_preserved') is True
            and grade.get('salience', 0) >= 4 and grade.get('clarity', 0) >= 4
            and grade.get('blocking_issues') == [])


def mechanical_errors(result: dict[str, Any]) -> list[str]:
    analysis = result.get('analysis') or {}
    summary = analysis.get('summary')
    if not summary:
        return [analysis.get('error') or 'No generated summary']
    text = summary['overview']
    errors = []
    if not text or not text.endswith('.') or any(ord(c) < 32 or ord(c) == 127 for c in text):
        errors.append('Empty, incomplete or control-character-containing post')
    if len(text + '\n' + result['source_url']) > 300:
        errors.append('Complete post exceeds 300 Unicode code points')
    if re.search(r'https?://', text):
        errors.append('Model inserted an extra URL in the prose')
    pages = result.get('pages', [])
    if not summary.get('requirements'):
        errors.append('No supporting evidence')
    for item in summary.get('requirements', []):
        page = item['page']
        if not 1 <= page <= len(pages):
            errors.append('Invalid citation page')
        elif len(item['evidence'].strip()) < 20 or ' '.join(item['evidence'].split()) not in ' '.join(pages[page - 1].split()):
            errors.append('Citation does not occur on source page')
    return errors


def validate_inventory(cases: dict[str, Any], results: list[dict[str, Any]], repeats: int) -> None:
    expected = {(case_id, repeat) for case_id in cases for repeat in range(1, repeats + 1)}
    actual = [(r['id'], r['repeat']) for r in results]
    if len(actual) != len(set(actual)):
        raise ValueError('Duplicate case/repeat results')
    if set(actual) != expected:
        raise ValueError(f'Incomplete run: missing={sorted(expected - set(actual))}, unexpected={sorted(set(actual) - expected)}')
    for key in ['model', 'workflow_sha256', 'pipeline_version', 'extractor_version', 'dataset_sha256']:
        if len({r.get(key) for r in results}) != 1:
            raise ValueError(f'Mixed {key} values; compare separate complete runs')


def run_metrics(results: list[dict[str, Any]]) -> dict[str, Any]:
    histories = []
    for result in results:
        history = [result]
        while isinstance(history[-1].get('quota_interrupted_attempt'), dict):
            history.append(history[-1]['quota_interrupted_attempt'])
        histories.append(history)
    steps = [step for history in histories for r in history for step in (r.get('analysis') or {}).get('trace', [])]
    elapsed = sorted(sum(r.get('elapsed_ms', 0) for r in history) / 1000 for history in histories)
    tokens = {key: sum((s.get('usage') or {}).get(key, 0) or 0 for s in steps)
              for key in ['prompt_tokens', 'completion_tokens', 'total_tokens']}
    costs = [((s.get('usage') or {}).get('provider_usage') or {}).get('cost') for s in steps]
    known_costs = [c for c in costs if isinstance(c, (int, float)) and not isinstance(c, bool) and math.isfinite(c) and c >= 0]
    return dict(api_calls=len(steps), usage=tokens, quota_interruptions=sum(len(h) - 1 for h in histories),
                reported_cost_usd=sum(known_costs) if len(known_costs) == len(steps) and steps else None,
                reported_cost_subtotal_usd=sum(known_costs) if known_costs else None,
                calls_without_reported_cost=len(steps) - len(known_costs),
                calls_without_usage=sum(not s.get('usage') for s in steps),
                trials_needing_repair=sum(any(s['round'] > 1 for s in trace)
                    or len({(s.get('stage', i), s['round']) for i, s in enumerate(trace)}) < len(trace)
                    for r in results if (trace := (r.get('analysis') or {}).get('trace', []))),
                median_seconds=round(statistics.median(elapsed), 2) if elapsed else None,
                p95_seconds=round(elapsed[math.ceil(len(elapsed) * .95) - 1], 2) if elapsed else None)


def review_fingerprint(case: dict[str, Any], pages: list[str], text: str, evidence: Any) -> str:
    payload = [PROMPT, case, pages, text, evidence]
    return hashlib.sha256(json.dumps(payload, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def validated_review(reviews: dict[str, Any], name: str, fingerprint: str) -> dict[str, Any]:
    grade = reviews.get('grades', {}).get(name)
    if not grade or grade.get('fingerprint') != fingerprint:
        raise ValueError('Missing or stale editorial review')
    if not reviews.get('reviewer') or not grade.get('rationale'):
        raise ValueError('Review needs attribution and a source-based rationale')
    if any(type(grade.get(k)) is not bool for k in ['accurate', 'qualifications_preserved']):
        raise ValueError('Review needs explicit accuracy and qualification verdicts')
    if any(type(grade.get(k)) is not int or not 1 <= grade[k] <= 5 for k in ['salience', 'clarity']):
        raise ValueError('Editorial scores must be integers from 1 to 5')
    if not isinstance(grade.get('blocking_issues'), list) or not all(isinstance(x, str) for x in grade['blocking_issues']):
        raise ValueError('Review needs a list of blocking issues')
    return grade


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--cases', type=Path, default=ROOT / 'cases.json')
    parser.add_argument('--reviews', type=Path, help='Local editorial grades; never calls an API')
    parser.add_argument('--calibrate', action='store_true', help='Export/score labelled rubric examples')
    parser.add_argument('--offline', action='store_true', help='Allow a successful exit for mechanical checks alone; never passes the editorial gate')
    parser.add_argument('--case', action='append', default=[])
    parser.add_argument('--split', action='append', default=[])
    parser.add_argument('--repeats', type=int, default=1)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if not 1 <= args.repeats <= 20:
        parser.error('--repeats must be 1–20')
    if args.offline and args.reviews:
        parser.error('--offline cannot be combined with --reviews')
    case_bytes = args.cases.read_bytes()
    cases = {c['id']: c for c in json.loads(case_bytes)
             if (not args.case or c['id'] in args.case) and (not args.split or c['split'] in args.split)}
    if not cases or set(args.case) - set(cases):
        parser.error('No matching cases, or unknown/excluded case IDs')
    for case in cases.values():
        if case.get('pdf') and hashlib.sha256((args.cases.parent / case['pdf']).read_bytes()).hexdigest() != case['sha256']:
            raise ValueError(f'Fixture changed: {case["id"]}')
    results = [json.loads(p.read_text()) for p in sorted(args.results.glob('*.json'))]
    results = [r for r in results if r.get('id') in cases]
    try:
        validate_inventory(cases, results, args.repeats)
        dataset_hash = hashlib.sha256(case_bytes).hexdigest()
        if any(r.get('dataset_sha256') != dataset_hash for r in results):
            raise ValueError('Results were generated with a different or unidentified dataset')
    except ValueError as error:
        parser.error(str(error))
    reviews = json.loads(args.reviews.read_text()) if args.reviews else {}
    jobs = []
    seen = set()
    for result in results:
        if args.calibrate:
            if result['id'] in seen:
                continue
            seen.add(result['id'])
            for i, c in enumerate(cases[result['id']]['calibration']):
                jobs.append((result, c['text'], f'{result["id"]}-calibration-{i}', c['expected_pass'], None))
        else:
            summary = (result.get('analysis') or {}).get('summary') or {}
            jobs.append((result, summary.get('overview', ''), f'{result["id"]}-{result["repeat"]}', None, summary.get('requirements')))

    packet_cases = {}
    scored: list[dict[str, Any]] = []
    for result, text, name, expected, evidence in jobs:
        case = cases[result['id']]
        fingerprint = review_fingerprint(case, result['pages'], text, evidence)
        entry = packet_cases.setdefault(result['id'], dict(project=case['project_name'], source_url=result['source_url'],
            gold_notes=case['gold'], pages=result['pages'], drafts=[]))
        entry['drafts'].append(dict(id=name, fingerprint=fingerprint, text=text, evidence=evidence))
        errors = [] if args.calibrate else mechanical_errors(result)
        grade = None
        if args.reviews and not errors:
            try:
                grade = validated_review(reviews, name, fingerprint)
            except ValueError as error:
                errors.append(str(error))
        passed = not errors and (quality_pass(grade) if grade is not None else not args.reviews)
        scored.append(dict(id=name, case_id=result['id'], text=text, source_url=result['source_url'],
            post_characters=len(text + '\n' + result['source_url']), evidence=evidence,
            passed=passed, errors=errors, grade=grade, expected_pass=expected,
            generator_model=result.get('model'), workflow_sha256=result.get('workflow_sha256'),
            dataset_sha256=result.get('dataset_sha256')))
    packet = dict(rubric=PROMPT, cases=packet_cases,
        review_format={'reviewer': 'Name/context of reviewer', 'grades': {'case-id-repeat': {
            'fingerprint': 'Copy from draft', 'accurate': True, 'qualifications_preserved': True,
            'salience': 4, 'clarity': 4, 'blocking_issues': [], 'rationale': 'Source-based explanation, not just a pass label'}}})
    packet_path = args.results / ('calibration-packet.json' if args.calibrate else 'review-packet.json')
    packet_path.write_text(json.dumps(packet, indent=2, ensure_ascii=False) + '\n')
    if args.calibrate:
        false_positives = sum(s['passed'] and s['expected_pass'] is False for s in scored)
        agreement = sum(s['passed'] == s['expected_pass'] for s in scored) / len(scored)
        success = bool(args.reviews) and false_positives == 0 and agreement >= .95 and not any(s['errors'] for s in scored)
        metrics = dict(editorial_evaluation=bool(args.reviews), agreement=agreement if args.reviews else None,
                       false_positives=false_positives if args.reviews else None, total=len(scored))
    else:
        success = bool(args.reviews) and all(s['passed'] for s in scored)
        metrics = dict(pass_rate=sum(s['passed'] for s in scored) / len(scored), total=len(scored),
            editorial_evaluation=bool(args.reviews), generation=run_metrics(results))
    output = args.output or args.results / ('calibration.json' if args.calibrate else 'scores.json')
    output.write_text(json.dumps(dict(metrics=metrics, success=success, reviewer=reviews.get('reviewer'),
        review_dataset_sha256=hashlib.sha256(case_bytes).hexdigest(),
        rubric_sha256=hashlib.sha256(PROMPT.encode()).hexdigest(), results=scored), indent=2, ensure_ascii=False) + '\n')
    print(json.dumps(metrics))
    print(f'Review packet: {packet_path}')
    mechanical_only_ok = args.offline and not args.calibrate and all(s['passed'] for s in scored)
    return 0 if success or mechanical_only_ok else 1


if __name__ == '__main__':
    sys.exit(main())
