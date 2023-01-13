// Copyright 2015 Google Inc. All rights reserved
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#ifndef STRUTIL_H_
#define STRUTIL_H_

#include <memory>
#include <string>
#include <string_view>
#include <vector>

// A class that can efficiently append to a string,
// and then concatenate all the appends together to one
// string at the end.
class StringBuilder {
public:
  StringBuilder() = default;
  explicit StringBuilder(const std::string&);
  StringBuilder(const StringBuilder&) = delete;
  StringBuilder(StringBuilder&&) = delete;

  StringBuilder& operator+=(const StringBuilder&);
  StringBuilder& operator+=(std::string&&);
  StringBuilder& operator+=(std::string_view);
  StringBuilder& operator+=(const char*);
  StringBuilder& operator+=(char);
  void append(const StringBuilder&);
  void append(std::string&&);
  void append(std::string_view);
  void append(const char*);
  void push_back(char);

  bool empty() const;

  const std::string& str() const;

private:
  mutable std::vector<std::shared_ptr<const std::string>> pieces_;
};

class WordScanner {
 public:
  struct Iterator {
    Iterator& operator++();
    std::string_view operator*() const;
    bool operator!=(const Iterator& r) const {
      return in != r.in || s != r.s || i != r.i;
    }

    const std::string_view* in;
    int s;
    int i;
  };

  explicit WordScanner(std::string_view in);

  Iterator begin() const;
  Iterator end() const;

  void Split(std::vector<std::string_view>* o);

 private:
  std::string_view in_;
};

class WordWriter {
 public:
  explicit WordWriter(std::string* o);
  explicit WordWriter(StringBuilder* o);
  void MaybeAddWhitespace();
  void Write(std::string_view s);

 private:
  std::string* out_ = nullptr;
  StringBuilder* out_builder_ = nullptr;
  bool needs_space_ = false;
};

// Temporary modifies s[s.size()] to '\0'.
class ScopedTerminator {
 public:
  explicit ScopedTerminator(std::string_view s);
  ~ScopedTerminator();

 private:
  std::string_view s_;
  char c_;
};

template <class String>
inline std::string JoinStrings(std::vector<String> v, const char* sep) {
  std::string r;
  for (std::string_view s : v) {
    if (!r.empty()) {
      r += sep;
    }
    r.append(s.begin(), s.end());
  }
  return r;
}

bool HasPrefix(std::string_view str, std::string_view prefix);

bool HasSuffix(std::string_view str, std::string_view suffix);

bool HasWord(std::string_view str, std::string_view w);

std::string_view TrimPrefix(std::string_view str, std::string_view prefix);

std::string_view TrimSuffix(std::string_view str, std::string_view suffix);

class Pattern {
 public:
  explicit Pattern(std::string_view pat);

  bool Match(std::string_view str) const;

  std::string_view Stem(std::string_view str) const;

  template<class String>
  void AppendSubst(std::string_view str,
                   std::string_view subst,
                   String* out) const {
    if (percent_index_ == std::string::npos) {
      if (str == pat_) {
        out->append(subst);
        return;
      } else {
        out->append(str);
        return;
      }
    }

    if (MatchImpl(str)) {
      size_t subst_percent_index = subst.find('%');
      if (subst_percent_index == std::string::npos) {
        out->append(subst);
        return;
      } else {
        out->append(subst.substr(0, subst_percent_index));
        out->append(str.substr(percent_index_, str.size() - pat_.size() + 1));
        out->append(subst.substr(subst_percent_index + 1));
        return;
      }
    }
    out->append(str);
  }

  template<class String>
  void AppendSubstRef(std::string_view str,
                      std::string_view subst,
                      String* out) const {
    if (percent_index_ != std::string::npos &&
        subst.find('%') != std::string::npos) {
      AppendSubst(str, subst, out);
      return;
    }
    std::string_view s = TrimSuffix(str, pat_);
    out->append(s);
    out->append(subst);
  }

 private:
  bool MatchImpl(std::string_view str) const;

  std::string_view pat_;
  size_t percent_index_;
};

std::string NoLineBreak(const std::string& s);

std::string_view TrimLeftSpace(std::string_view s);
std::string_view TrimRightSpace(std::string_view s);
std::string_view TrimSpace(std::string_view s);

std::string_view Dirname(std::string_view s);
std::string_view Basename(std::string_view s);
std::string_view GetExt(std::string_view s);
std::string_view StripExt(std::string_view s);
void NormalizePath(std::string* o);
void AbsPath(std::string_view s, std::string* o);

size_t FindOutsideParen(std::string_view s, char c);
size_t FindTwoOutsideParen(std::string_view s, char c1, char c2);
size_t FindThreeOutsideParen(std::string_view s, char c1, char c2, char c3);

size_t FindEndOfLine(std::string_view s, size_t e, size_t* lf_cnt);

// Strip leading sequences of './' from file names, so that ./file
// and file are considered to be the same file.
// From http://www.gnu.org/software/make/manual/make.html#Features
std::string_view TrimLeadingCurdir(std::string_view s);

void FormatForCommandSubstitution(std::string* s);

std::string SortWordsInString(std::string_view s);

std::string ConcatDir(std::string_view b, std::string_view n);

std::string EchoEscape(const std::string& str);

void EscapeShell(std::string* s);

bool IsInteger(std::string_view s);

#endif  // STRUTIL_H_
