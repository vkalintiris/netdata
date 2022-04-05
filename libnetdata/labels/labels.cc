#include "labels.h"

#include <map>
#include <string>

class Label {
public:
    Label() = default;

    Label(std::string Key, std::string Value, label_source_t Source)
        : Key(Key), Value(Value), Source(Source) {}

    const char *getKey() const {
        return Key.c_str();
    }

    const char *getValue() const {
        return Value.c_str();
    }

    label_source_t getSource() const {
        return Source;
    }

private:
    std::string Key;
    std::string Value;
    label_source_t Source;
};

class LabelList {
public:
    void addLabel(std::string Key, std::string Value, label_source_t Source) {
        LM[Key] = Label(Key, Value, Source);
    }

    Label *lookupKey(std::string Key) {
        auto It = LM.find(Key);
        return It != LM.end() ? &It->second : nullptr;
    }

    Label *lookupKeyList(const char *KeyList) {
        Label *Res = nullptr;

        SIMPLE_PATTERN *SP = simple_pattern_create(KeyList, ",|\t\r\n\f\v", SIMPLE_PATTERN_EXACT);
        for (auto It : LM) {
            Label *L = &It.second;

            if (simple_pattern_matches(SP, L->getKey())) {
                Res = L;
                break;
            }
        }
        simple_pattern_free(SP);

        return Res;
    }

    bool foreach(label_callback_t CB, void *Data) const {
        for (auto It : LM) {
            if (CB(&It.second, Data))
                return true;
        }

        return false;
    }

    void update(const LabelList *Other) {
        for (auto &P : Other->LM) {
            LM[P.first] = P.second;
        }
    }

    size_t size() const {
        return LM.size();
    }

    void clear() {
        LM.clear();
    }

    void toJsonBuffer(BUFFER *wb, const char *kv_format, const char *kv_separator, size_t indentation) const {
        std::string tabs(indentation > 10 ? 10 : indentation, '\t');

        int count = 0;
        for (auto It : LM) {
            Label *L = &It.second;

            char value[CONFIG_MAX_VALUE * 2 + 1];
            sanitize_json_string(value, L->getValue(), CONFIG_MAX_VALUE * 2);

            if (count > 0)
                buffer_strcat(wb, kv_separator);
            buffer_strcat(wb, tabs.c_str());
            buffer_sprintf(wb, kv_format, L->getKey(), value);

            count++;
        }
    }

    void print() const {
        for (auto It : LM) {
            error("GVD: Key = %s", It.first.c_str());
        }
    }

private:
    std::map<std::string, Label> LM;
};

/*
 * C-API for Label
 */
const char *label_key(label_t label) {
    Label *L = static_cast<Label *>(label);
    return L->getKey();
}

const char *label_value(label_t label) {
    Label *L = static_cast<Label *>(label);
    return L->getValue();
}

label_source_t label_source(label_t label) {
    Label *L = static_cast<Label *>(label);
    return L->getSource();
}

/*
 * C-API for LabelList
 */
label_list_t label_list_new() {
    return new LabelList;
}

 void label_list_delete(label_list_t List) {
    LabelList *LL = static_cast<LabelList *>(List);
    delete LL;
}

void label_list_clear(label_list_t list) {
    LabelList *LL = static_cast<LabelList *>(list);
    LL->clear();
}

void label_list_add(label_list_t list, const char *key, const char *value, label_source_t source) {
    LabelList *LL = static_cast<LabelList *>(list);
    LL->addLabel(key, value, source);
}

label_t label_list_lookup_key(label_list_t list, const char *key) {
    LabelList *LL = static_cast<LabelList *>(list);
    return LL->lookupKey(key);
}

label_t label_list_lookup_keylist(label_list_t list, const char *keylist) {
    LabelList *LL = static_cast<LabelList *>(list);
    return LL->lookupKeyList(keylist);
}

bool label_list_foreach(label_list_t list, label_callback_t cb, void *cb_data) {
    if (!list)
        return false;

    LabelList *LL = static_cast<LabelList *>(list);
    return LL->foreach(cb, cb_data);
}

void label_list_update(label_list_t dst, const label_list_t src) {
    LabelList *DstLL = static_cast<LabelList *>(dst);
    const LabelList *SrcLL = static_cast<const LabelList *>(src);

    DstLL->update(SrcLL);
}

void label_list_to_json_buffer(label_list_t list, BUFFER *wb,
                                   const char *kv_format,
                                   const char *kv_separator,
                                   size_t indentation)
{
    LabelList *LL = static_cast<LabelList *>(list);
    LL->toJsonBuffer(wb, kv_format, kv_separator, indentation);
}

void label_list_print(label_list_t list) {
    const LabelList *LL = static_cast<const LabelList *>(list);

    LL->print();
}

size_t label_list_size(label_list_t list) {
    if (!list)
        return 0;

    const LabelList *LL = static_cast<const LabelList *>(list);
    return LL->size();
}
